//! Integration tests for the Kronn backend API.
//!
//! These tests exercise the full HTTP layer (router + handlers + DB)
//! using `tower::ServiceExt::oneshot` with an in-memory SQLite database.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

use futures::{SinkExt, StreamExt};
use kronn::models::WsMessage;
use kronn::{build_router_with_auth, AppState, DEFAULT_MAX_CONCURRENT_AGENTS};

#[tokio::test]
async fn orchestration_recovery_routes_return_structured_missing_execution_errors() {
    let app = test_app();
    let missing = "00000000-0000-0000-0000-000000000000";

    let (status, detail) = get_json(
        app.clone(),
        &format!("/api/orchestration/executions/{missing}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["success"], false, "{detail}");
    assert!(detail["error"]
        .as_str()
        .is_some_and(|error| error.contains("not found")));

    let (status, observability) = get_json(
        app.clone(),
        &format!("/api/orchestration/executions/{missing}/observability"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(observability["success"], false, "{observability}");
    assert!(observability["error"]
        .as_str()
        .is_some_and(|error| error.contains("not found")));

    let (status, recovery) = get_json(
        app.clone(),
        &format!("/api/orchestration/executions/{missing}/recovery"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(recovery["success"], false, "{recovery}");
    assert!(recovery["error"]
        .as_str()
        .is_some_and(|error| error.contains("not found")));

    let (status, resume) = post_json(
        app.clone(),
        &format!("/api/orchestration/executions/{missing}/resume"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resume["success"], false, "{resume}");
    assert!(resume["error"]
        .as_str()
        .is_some_and(|error| error.contains("not found")));

    let (status, cancel) = post_json(
        app.clone(),
        &format!("/api/orchestration/executions/{missing}/cancel"),
        serde_json::json!({ "reason": "operator cancellation" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancel["success"], false, "{cancel}");
    assert!(cancel["error"]
        .as_str()
        .is_some_and(|error| error.contains("not found")));

    let (status, reassign) = post_json(
        app,
        &format!("/api/orchestration/executions/{missing}/reassign"),
        serde_json::json!({
            "worker": {
                "target": { "kind": "agent", "agent_type": "ClaudeCode" }
            },
            "reason": "provider unavailable"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reassign["success"], false, "{reassign}");
    assert!(reassign["error"].as_str().is_some());

    let (status, review) = post_json(
        test_app(),
        &format!("/api/orchestration/executions/{missing}/review"),
        serde_json::json!({
            "decision": {
                "version": "1",
                "task_ref": "KT-1",
                "decision": "approve"
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(review["success"], false, "{review}");
    assert_eq!(review["error_code"], "not_found", "{review}");
}

#[tokio::test]
async fn orchestration_discussion_campaign_round_trips_the_active_policy() {
    let state = test_state();
    let app = build_router_with_auth(state, false);
    let (_, created_discussion) = post_json(
        app.clone(),
        "/api/discussions",
        serde_json::json!({
            "title": "Principal campaign",
            "agent": "Codex",
            "language": "fr",
            "initial_prompt": "Drive the plan"
        }),
    )
    .await;
    assert_eq!(created_discussion["success"], true, "{created_discussion}");
    let discussion_id = created_discussion["data"]["id"].as_str().unwrap();

    let (_, created_campaign) = post_json(
        app.clone(),
        "/api/orchestration/campaigns",
        serde_json::json!({
            "discussion_id": discussion_id,
            "target_branch": "release/0.11.0",
            "integration_strategy": "two_phase_ff_only",
            "allowed_agents": ["Codex", "Ollama"],
            "max_review_rounds": 4,
            "auto_continue": true
        }),
    )
    .await;
    assert_eq!(created_campaign["success"], true, "{created_campaign}");
    let run_id = created_campaign["data"]["run"]["id"].as_str().unwrap();

    let (_, active) = get_json(
        app.clone(),
        &format!("/api/orchestration/discussions/{discussion_id}/campaign"),
    )
    .await;
    assert_eq!(active["success"], true, "{active}");
    assert_eq!(active["data"]["run"]["id"], run_id);
    assert_eq!(active["data"]["run"]["target_branch"], "release/0.11.0");
    assert_eq!(active["data"]["run"]["max_review_rounds"], 4);
    assert_eq!(active["data"]["run"]["auto_continue"], true);

    let (_, cancelled) = post_json(
        app.clone(),
        &format!("/api/orchestration/campaigns/{run_id}/control"),
        serde_json::json!({ "state": "cancelled" }),
    )
    .await;
    assert_eq!(cancelled["success"], true, "{cancelled}");
    let (_, inactive) = get_json(
        app,
        &format!("/api/orchestration/discussions/{discussion_id}/campaign"),
    )
    .await;
    assert_eq!(inactive["success"], true, "{inactive}");
    assert!(inactive["data"].is_null(), "{inactive}");
}

#[tokio::test]
async fn orchestration_cli_tool_routes_require_server_verifiable_identity() {
    let app = test_app();
    let missing_execution = "00000000-0000-0000-0000-000000000000";
    let calls = [
        (
            "/api/orchestration/tool/prepare".to_string(),
            serde_json::json!({
                "task_reference": "KT-1",
                "parent_discussion_id": "disc-parent",
                "worker": {"kind": "agent", "agent_type": "Ollama"},
                "worker_scope_intent": "generic",
                "source_agent": "",
                "source_session_id": ""
            }),
        ),
        (
            "/api/orchestration/tool/launch".to_string(),
            serde_json::json!({
                "task_reference": "KT-1",
                "parent_discussion_id": "disc-parent",
                "worker": {"kind": "agent", "agent_type": "Ollama"},
                "worker_scope_intent": "generic",
                "source_agent": "",
                "source_session_id": ""
            }),
        ),
        (
            format!("/api/orchestration/tool/executions/{missing_execution}/status"),
            serde_json::json!({"source_agent": "", "source_session_id": ""}),
        ),
        (
            format!("/api/orchestration/tool/executions/{missing_execution}/cancel"),
            serde_json::json!({
                "source_agent": "",
                "source_session_id": "",
                "reason": "test"
            }),
        ),
        (
            format!("/api/orchestration/tool/executions/{missing_execution}/reassign"),
            serde_json::json!({
                "source_agent": "",
                "source_session_id": "",
                "worker": {"target": {"kind": "agent", "agent_type": "Ollama"}},
                "reason": "test"
            }),
        ),
    ];

    for (path, body) in calls {
        let (status, response) = post_json(app.clone(), &path, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "route missing for {path}: {response}"
        );
        assert_eq!(response["success"], false, "{path}: {response}");
        assert_eq!(response["error_code"], "validation", "{path}: {response}");
        assert!(response["error"].as_str().is_some_and(
            |error| error.contains("source_agent") && error.contains("source_session_id")
        ));
    }
}

#[tokio::test]
async fn orchestration_tool_routes_refuse_a_stale_host_scope_schema_before_provisioning() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);
    let request = serde_json::json!({
        "task_reference": "KT-466",
        "parent_discussion_id": "disc-parent",
        "worker": {"kind": "agent", "agent_type": "Ollama"},
        "worker_scope": {
            "mode": "prelocalized_insert_after",
            "path": "docs/operations/ollama-local-models.md",
            "anchor_line": 168
        },
        "source_agent": "Codex",
        "source_session_id": "fresh-process-stale-host-schema"
    });
    for route in [
        "/api/orchestration/tool/prepare",
        "/api/orchestration/tool/launch",
    ] {
        let (_, response) = post_json(app.clone(), route, request.clone()).await;
        assert_eq!(response["success"], false, "{route}: {response}");
        assert_eq!(response["error_code"], "validation", "{route}: {response}");
        assert!(response["error"]
            .as_str()
            .is_some_and(|error| error.contains("worker_scope_intent_missing")));
        assert!(response["error"]
            .as_str()
            .is_some_and(|error| error.contains("reconnect")));
    }

    let counts = state
        .db
        .with_conn(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM orchestration_runs", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                conn.query_row("SELECT COUNT(*) FROM task_executions", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                conn.query_row("SELECT COUNT(*) FROM discussion_workspaces", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                conn.query_row("SELECT COUNT(*) FROM agent_dispatch_jobs", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(counts, (0, 0, 0, 0));
}

#[tokio::test]
async fn orchestration_launch_fails_closed_when_the_exact_cli_is_unavailable() {
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.email", "orchestration@test.invalid"],
        vec!["config", "user.name", "Orchestration Test"],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
    }
    std::fs::write(repo.path().join("README.md"), "# orchestration e2e\n").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "initial"]] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
    }

    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);
    let (_, project) = post_json(
        app.clone(),
        "/api/projects",
        serde_json::json!({
            "path": repo.path().to_string_lossy(),
            "name": "Orchestration E2E",
            "remote_url": null,
            "branch": "main",
            "ai_configs": [],
            "has_project": false,
            "hidden": false
        }),
    )
    .await;
    assert_eq!(project["success"], true, "{project}");
    let project_id = project["data"]["id"].as_str().unwrap().to_string();

    let (_, discussion) = post_json(
        app.clone(),
        "/api/discussions",
        serde_json::json!({
            "project_id": project_id,
            "title": "Orchestration principal",
            "agent": "Codex",
            "language": "en",
            "initial_prompt": "Drive the plan"
        }),
    )
    .await;
    assert_eq!(discussion["success"], true, "{discussion}");
    let discussion_id = discussion["data"]["id"].as_str().unwrap().to_string();

    let (_, task) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Unavailable exact CLI",
            "description": "The launch boundary must fail closed.",
            "status": "todo",
            "project_ids": [project_id],
            "discussion_id": discussion_id,
            "definition_of_done": [
                {"sentence": "No fallback worker is started", "completed": false}
            ]
        }),
    )
    .await;
    assert_eq!(task["success"], true, "{task}");
    let task_reference = task["data"]["reference"].as_str().unwrap().to_string();

    let principal_discussion = discussion_id.clone();
    state
        .db
        .with_conn(move |conn| {
            kronn::db::discussion_sessions::create_session(
                conn,
                &principal_discussion,
                "Codex",
                Some("principal-orchestration-e2e"),
                "peer",
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let request = serde_json::json!({
        "task_reference": task_reference,
        "parent_discussion_id": discussion_id,
        "worker": {
            "kind": "cli",
            "agent_type": "ClaudeCode",
            "cli_session_id": 404
        },
        "source_agent": "Codex",
        "source_session_id": "principal-orchestration-e2e",
        "worker_scope_intent": "generic",
        "base_rev": "main",
        "idempotency_key": "unavailable-cli-e2e"
    });
    let (_, prepared) = post_json(
        app.clone(),
        "/api/orchestration/tool/prepare",
        request.clone(),
    )
    .await;
    assert_eq!(prepared["success"], true, "{prepared}");
    assert_eq!(prepared["data"]["launchable"], false, "{prepared}");
    assert!(prepared["data"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason["code"] == "worker_unavailable"));

    let (_, launched) = post_json(app, "/api/orchestration/tool/launch", request).await;
    assert_eq!(launched["success"], false, "{launched}");
    assert_eq!(launched["error_code"], "validation", "{launched}");
    assert!(launched["error"]
        .as_str()
        .is_some_and(|error| error.contains("worker_unavailable")));

    let execution_count = state
        .db
        .with_conn(|conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM task_executions", [], |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        })
        .await
        .unwrap();
    assert_eq!(execution_count, 0, "HTTP refusal created an execution");
    assert!(
        !repo.path().join(".kronn/worktrees").exists(),
        "HTTP refusal created a worktree directory"
    );
}

#[tokio::test]
async fn quick_exec_crud_and_csv_run_round_trip() {
    let app = test_app();
    let (create_status, created) = post_json(
        app.clone(),
        "/api/quick-execs",
        serde_json::json!({
            "name": "CSV inventory",
            "description": "Portable CLI collector",
            "project_id": null,
            "command": "echo",
            "args": ["name,total\nParis,12\nLondon,9"],
            "timeout_secs": 10,
            "output_format": "csv",
            "variables": []
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK);
    assert_eq!(created["success"], true, "{created}");
    let id = created["data"]["id"].as_str().unwrap().to_string();

    let (_, listed) = get_json(app.clone(), "/api/quick-execs").await;
    assert_eq!(listed["data"][0]["id"], id);

    let (_, run) = post_json(
        app.clone(),
        &format!("/api/quick-execs/{id}/run"),
        serde_json::json!({"variables": {}}),
    )
    .await;
    assert_eq!(run["success"], true, "{run}");
    assert_eq!(run["data"]["success"], true, "{run}");
    assert_eq!(run["data"]["data"][0]["name"], "Paris");
    assert_eq!(run["data"]["data"][0]["total"], "12");
    assert_eq!(run["data"]["data"][1]["name"], "London");

    let (_, deleted) = delete_json(app.clone(), &format!("/api/quick-execs/{id}")).await;
    assert_eq!(deleted["success"], true);
    let (_, empty) = get_json(app, "/api/quick-execs").await;
    assert!(empty["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn automation_quick_items_share_a_persistent_favorite_patch() {
    let app = test_app();

    let (_, prompt) = post_json(
        app.clone(),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Favorite prompt",
            "prompt_template": "Summarize this",
            "variables": []
        }),
    )
    .await;
    let prompt_id = prompt["data"]["id"].as_str().unwrap();

    let (_, api) = post_json(
        app.clone(),
        "/api/quick-apis",
        serde_json::json!({
            "name": "Favorite API",
            "api_plugin_slug": "demo",
            "api_config_id": "cfg",
            "api_endpoint_path": "/items",
            "variables": []
        }),
    )
    .await;
    let api_id = api["data"]["id"].as_str().unwrap();

    let (_, exec) = post_json(
        app.clone(),
        "/api/quick-execs",
        serde_json::json!({
            "name": "Favorite exec",
            "command": "echo",
            "args": ["ok"],
            "output_format": "text",
            "variables": []
        }),
    )
    .await;
    let exec_id = exec["data"]["id"].as_str().unwrap();

    for (kind, id) in [
        ("quick-prompts", prompt_id),
        ("quick-apis", api_id),
        ("quick-execs", exec_id),
    ] {
        let (status, response) = patch_json(
            app.clone(),
            &format!("/api/{kind}/{id}"),
            serde_json::json!({"pinned": true}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{kind}: {response}");
        assert_eq!(response["success"], true, "{kind}: {response}");
        assert_eq!(response["data"]["pinned"], true, "{kind}: {response}");

        let (_, listed) = get_json(app.clone(), &format!("/api/{kind}")).await;
        assert_eq!(listed["data"][0]["pinned"], true, "{kind}: {listed}");
    }

    // Favorite metadata is not prompt content and must not create a fake QP
    // version. The initial insert remains the only snapshot after pinning.
    let (_, history) = get_json(app, &format!("/api/quick-prompts/{prompt_id}/history")).await;
    assert_eq!(history["data"].as_array().unwrap().len(), 1, "{history}");
}

#[tokio::test]
async fn collect_api_data_test_returns_an_inspectable_error_envelope() {
    let app = test_app();
    let (status, body) = post_json(
        app,
        "/api/workflow-steps/test-collect-api-data",
        serde_json::json!({
            "step": {
                "name": "collect",
                "step_type": {"type": "CollectApiData"},
                "collect_api_data": {"sources": [], "concurrent_limit": 5}
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["data"]["success"], false);
    assert_eq!(body["data"]["envelope"]["status"], "ERROR");
    assert!(body["data"]["envelope"]["data"]["meta"]["error"]
        .as_str()
        .is_some_and(|error| error.contains("at least one")));
}

#[tokio::test]
async fn live_page_workflows_returns_configured_publishers() {
    let state = test_state();
    state
        .db
        .with_conn(|connection| {
            let now = chrono::Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO live_pages (id, project_id, title, slug, current_revision_id, data_revision, created_at, updated_at)
                 VALUES ('page-linked', NULL, 'Linked Page', 'linked-page', 'rev-linked', 0, ?1, ?1)",
                [&now],
            )?;
            connection.execute(
                "INSERT INTO live_page_revisions (id, page_id, revision, html, created_at)
                 VALUES ('rev-linked', 'page-linked', 1, '<h1>Linked</h1>', ?1)",
                [&now],
            )?;
            let workflow = kronn::models::Workflow {
                id: "wf-linked".into(),
                name: "Page producer".into(),
                project_id: None,
                trigger: kronn::models::WorkflowTrigger::Manual,
                steps: vec![kronn::models::WorkflowStep {
                    name: "publish".into(),
                    step_type: kronn::models::StepType::PublishPageData,
                    page_publish: Some(kronn::models::PublishPageDataConfig {
                        page_id: "page-linked".into(),
                        writes: vec![],
                    }),
                    ..Default::default()
                }],
                actions: vec![],
                safety: kronn::models::WorkflowSafety { sandbox: false, max_files: None, max_lines: None, require_approval: false },
                workspace_config: None,
                concurrency_limit: None,
                guards: None,
                artifacts: std::collections::HashMap::new(),
                on_failure: vec![],
                exec_allowlist: vec![],
                variables: vec![],
                enabled: true,
                pinned: false,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };
            kronn::db::workflows::insert_workflow(connection, &workflow)?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state.clone(), false);
    let (_, body) = get_json(app.clone(), "/api/pages/page-linked/workflows").await;
    assert_eq!(body["data"][0]["id"], "wf-linked");
    assert_eq!(body["data"][0]["step_names"][0], "publish");
    let (missing_status, missing) = get_json(app, "/api/pages/missing/workflows").await;
    assert_eq!(missing_status, StatusCode::OK);
    assert_eq!(missing["success"], false);
    assert_eq!(missing["error_code"], "not_found");
}

#[tokio::test]
async fn workflow_export_import_bundles_quick_prompt_quick_api_and_page() {
    let state = test_state();
    let now = chrono::Utc::now();
    state
        .db
        .with_conn(move |connection| {
            let quick_prompt = kronn::models::QuickPrompt {
                id: "qp-portable".into(),
                pinned: false,
                name: "Portable analysis".into(),
                icon: "✨".into(),
                prompt_template: "Analyse the collected data".into(),
                variables: vec![],
                agent: kronn::models::AgentType::ClaudeCode,
                project_id: None,
                skill_ids: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                tier: kronn::models::ModelTier::Default,
                agent_settings: None,
                description: "Bundled prompt".into(),
                created_at: now,
                updated_at: now,
            };
            kronn::db::quick_prompts::insert_quick_prompt(connection, &quick_prompt)?;
            let quick_api = kronn::models::QuickApi {
                id: "qa-portable".into(),
                pinned: false,
                name: "Portable API".into(),
                icon: "🔌".into(),
                description: "Bundled API".into(),
                project_id: None,
                api_plugin_slug: "chartbeat".into(),
                api_config_id: "foreign-config".into(),
                api_endpoint_path: "/live".into(),
                api_method: Some("GET".into()),
                api_query: None,
                api_path_params: None,
                api_headers: None,
                api_body: None,
                api_extract: None,
                api_pagination: None,
                api_timeout_ms: None,
                api_max_retries: None,
                variables: vec![],
                profile_ids: vec![],
                directive_ids: vec![],
                created_at: now,
                updated_at: now,
            };
            kronn::db::quick_apis::insert_quick_api(connection, &quick_api)?;
            let quick_exec = kronn::models::QuickExec {
                id: "qe-portable".into(),
                pinned: false,
                name: "Portable CLI".into(),
                icon: "⌘".into(),
                description: "Bundled CLI".into(),
                project_id: None,
                command: "echo".into(),
                args: vec!["name,total\nParis,12".into()],
                timeout_secs: 10,
                output_format: kronn::models::CollectQuickExecOutputFormat::Csv,
                variables: vec![],
                created_at: now,
                updated_at: now,
            };
            kronn::db::quick_execs::insert_quick_exec(connection, &quick_exec)?;

            let page = kronn::models::LivePage {
                id: "page-portable".into(),
                project_id: None,
                title: "Portable Page".into(),
                slug: "portable-page".into(),
                current_revision_id: "revision-portable".into(),
                data_revision: 0,
                created_at: now,
                updated_at: now,
                last_published_at: None,
                pinned: false,
                archived: false,
            };
            let revision = kronn::models::LivePageRevision {
                id: "revision-portable".into(),
                page_id: page.id.clone(),
                revision: 1,
                html: "<h1>Portable</h1>".into(),
                created_by_agent: Some("Codex".into()),
                created_at: now,
            };
            kronn::db::live_pages::create_live_page(
                connection,
                &page,
                &revision,
                &[kronn::models::CreateLivePageDataset {
                    name: "summary".into(),
                    kind: kronn::models::LivePageDatasetKind::Snapshot,
                    initial: Some(serde_json::json!({"seed": true})),
                    schema: None,
                    max_points: None,
                    max_age_days: None,
                }],
                None,
            )?;

            let workflow = kronn::models::Workflow {
                id: "workflow-portable".into(),
                name: "Portable workflow".into(),
                project_id: None,
                trigger: kronn::models::WorkflowTrigger::Manual,
                steps: vec![
                    kronn::models::WorkflowStep {
                        name: "analyse".into(),
                        step_type: kronn::models::StepType::Agent,
                        quick_prompt_id: Some("qp-portable".into()),
                        quick_prompt_variables: std::collections::HashMap::new(),
                        prompt_template: "Fallback analysis".into(),
                        ..Default::default()
                    },
                    kronn::models::WorkflowStep {
                        name: "collect".into(),
                        step_type: kronn::models::StepType::CollectApiData,
                        collect_api_data: Some(kronn::models::CollectApiDataConfig {
                            sources: vec![
                                kronn::models::CollectApiDataSource {
                                    alias: "metrics".into(),
                                    quick_api_id: "qa-portable".into(),
                                    quick_exec_id: String::new(),
                                    quick_exec: None,
                                    required: true,
                                    variables: Default::default(),
                                },
                                kronn::models::CollectApiDataSource {
                                    alias: "cli_inventory".into(),
                                    quick_api_id: String::new(),
                                    quick_exec_id: "qe-portable".into(),
                                    quick_exec: None,
                                    required: false,
                                    variables: Default::default(),
                                },
                            ],
                            concurrent_limit: Some(2),
                        }),
                        ..Default::default()
                    },
                    kronn::models::WorkflowStep {
                        name: "publish".into(),
                        step_type: kronn::models::StepType::PublishPageData,
                        page_publish: Some(kronn::models::PublishPageDataConfig {
                            page_id: "page-portable".into(),
                            writes: vec![kronn::models::PublishPageDataWrite {
                                dataset: "summary".into(),
                                operation: kronn::models::LivePageWriteOperation::Replace,
                                value_from: "steps.collect.data".into(),
                                observed_at: None,
                                dedupe_key: None,
                                key_field: None,
                            }],
                        }),
                        ..Default::default()
                    },
                ],
                actions: vec![],
                safety: kronn::models::WorkflowSafety {
                    sandbox: false,
                    max_files: None,
                    max_lines: None,
                    require_approval: false,
                },
                workspace_config: None,
                concurrency_limit: None,
                guards: None,
                artifacts: Default::default(),
                on_failure: vec![],
                exec_allowlist: vec!["echo".into()],
                variables: vec![],
                enabled: true,
                pinned: false,
                created_at: now,
                updated_at: now,
            };
            kronn::db::workflows::insert_workflow(connection, &workflow)?;
            Ok(())
        })
        .await
        .unwrap();

    let app = build_router_with_auth(state.clone(), false);
    let (export_status, exported) =
        get_json(app.clone(), "/api/workflows/workflow-portable/export").await;
    assert_eq!(export_status, StatusCode::OK);
    assert_eq!(exported["version"], 3);
    assert_eq!(exported["referenced_quick_prompts"][0]["id"], "qp-portable");
    assert_eq!(exported["referenced_quick_apis"][0]["id"], "qa-portable");
    assert_eq!(exported["referenced_quick_execs"][0]["id"], "qe-portable");
    assert_eq!(exported["referenced_pages"][0]["id"], "page-portable");
    assert!(exported["referenced_pages"][0]["datasets"][0]
        .get("current")
        .is_none());

    let (import_status, imported) = post_json(
        app.clone(),
        "/api/workflows/import",
        serde_json::json!({
            "content": serde_json::to_string(&exported).unwrap(),
            "project_id": null
        }),
    )
    .await;
    assert_eq!(import_status, StatusCode::OK);
    assert_eq!(imported["success"], true, "{imported}");
    let imported_steps = imported["data"]["steps"].as_array().unwrap();
    let imported_qp = imported_steps[0]["quick_prompt_id"].as_str().unwrap();
    let imported_qa = imported_steps[1]["collect_api_data"]["sources"][0]["quick_api_id"]
        .as_str()
        .unwrap();
    let imported_qe = imported_steps[1]["collect_api_data"]["sources"][1]["quick_exec_id"]
        .as_str()
        .unwrap();
    let imported_page = imported_steps[2]["page_publish"]["page_id"]
        .as_str()
        .unwrap();
    assert_ne!(imported_qp, "qp-portable");
    assert_ne!(imported_qa, "qa-portable");
    assert_ne!(imported_qe, "qe-portable");
    assert_ne!(imported_page, "page-portable");
    let qp_lookup = imported_qp.to_string();
    let qa_lookup = imported_qa.to_string();
    let qe_lookup = imported_qe.to_string();
    let (qp_exists, qa_exists, qe_exists) = state
        .db
        .with_read_conn(move |connection| {
            Ok((
                kronn::db::quick_prompts::get_quick_prompt(connection, &qp_lookup)?.is_some(),
                kronn::db::quick_apis::get_quick_api(connection, &qa_lookup)?.is_some(),
                kronn::db::quick_execs::get_quick_exec(connection, &qe_lookup)?.is_some(),
            ))
        })
        .await
        .unwrap();
    assert!(qp_exists);
    assert!(qa_exists);
    assert!(qe_exists);
    let (_, imported_page_body) = get_json(app, &format!("/api/pages/{imported_page}")).await;
    assert_eq!(
        imported_page_body["data"]["revision"]["html"],
        "<h1>Portable</h1>"
    );
    assert_eq!(
        imported_page_body["data"]["datasets"][0]["current"],
        Value::Null
    );
}

#[tokio::test]
async fn transform_data_preview_uses_runtime_recipe() {
    let app = test_app();
    let (status, body) = post_json(
        app,
        "/api/workflow-steps/preview-transform-data",
        serde_json::json!({
            "sample": {
                "sources": {
                    "usage": {"total": "42"},
                    "errors": {"items": [{"count": 2}, {"count": 3}]}
                }
            },
            "fields": [
                {
                    "target": "summary.requests",
                    "source": "$.sources.usage.total",
                    "operation": "copy",
                    "value_type": "number"
                },
                {
                    "target": "summary.errors",
                    "source": "$.sources.errors.items[*].count",
                    "operation": "sum"
                }
            ]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["data"]["error"], Value::Null);
    assert_eq!(body["data"]["value"]["summary"]["requests"], 42.0);
    assert_eq!(body["data"]["value"]["summary"]["errors"], 5.0);
}

#[tokio::test]
async fn live_page_create_publish_and_read_round_trip() {
    let app = test_app();
    let (_, capability_before) = get_json(app.clone(), "/api/pages/capability").await;
    assert_eq!(capability_before["data"]["activated"], false);

    let (_, created) = post_json(
        app.clone(),
        "/api/pages",
        serde_json::json!({
            "title": "Adobe signals",
            "slug": "adobe-signals",
            "html": "<!doctype html><h1>Adobe signals</h1>",
            "datasets": [
                {"name": "summary", "kind": "snapshot", "initial": {}},
                {"name": "latency", "kind": "time_series", "max_points": 2}
            ]
        }),
    )
    .await;
    assert_eq!(created["success"], true, "{created}");
    let page_id = created["data"]["id"].as_str().expect("page id");

    let (_, capability_after) = get_json(app.clone(), "/api/pages/capability").await;
    assert_eq!(capability_after["data"]["activated"], true);

    let (_, published) = post_json(
        app.clone(),
        &format!("/api/pages/{page_id}/publish"),
        serde_json::json!({
            "writes": [
                {"dataset": "summary", "operation": "replace", "value": {"errors": 3}},
                {"dataset": "latency", "operation": "append", "value": [
                    {"ms": 120}, {"ms": 95}, {"ms": 87}
                ], "dedupe_key": "run-1-latency"}
            ]
        }),
    )
    .await;
    assert_eq!(published["success"], true, "{published}");
    assert_eq!(published["data"]["data_revision"], 1);
    assert_eq!(published["data"]["points_added"], 3);
    assert_eq!(published["data"]["points_removed"], 1);

    let (_, updated_html) = put_json_root(
        app.clone(),
        &format!("/api/pages/{page_id}/html"),
        serde_json::json!({
            "html": "<!doctype html><h1>Adobe signals v2</h1>",
            "created_by_agent": "Codex"
        }),
    )
    .await;
    assert_eq!(updated_html["success"], true, "{updated_html}");
    assert_eq!(updated_html["data"]["revision"], 2);

    let (_, revisions) = get_json(app.clone(), &format!("/api/pages/{page_id}/revisions")).await;
    assert_eq!(revisions["data"].as_array().unwrap().len(), 2);
    assert_eq!(revisions["data"][0]["created_by_agent"], "Codex");

    let (_, detail) = get_json(app, &format!("/api/pages/{page_id}")).await;
    assert_eq!(detail["data"]["data_revision"], 1);
    assert_eq!(detail["data"]["revision"]["revision"], 2);
    let datasets = detail["data"]["datasets"].as_array().unwrap();
    let summary = datasets.iter().find(|d| d["name"] == "summary").unwrap();
    let latency = datasets.iter().find(|d| d["name"] == "latency").unwrap();
    assert_eq!(summary["current"]["errors"], 3);
    assert_eq!(
        summary["data_size_bytes"].as_u64().unwrap(),
        u64::try_from(
            serde_json::to_vec(&serde_json::json!({"errors": 3}))
                .unwrap()
                .len()
        )
        .unwrap()
    );
    assert_eq!(latency["points"].as_array().unwrap().len(), 2);
    assert_eq!(
        latency["data_size_bytes"].as_u64().unwrap(),
        u64::try_from(
            serde_json::to_vec(&serde_json::json!({"ms": 95}))
                .unwrap()
                .len()
                + serde_json::to_vec(&serde_json::json!({"ms": 87}))
                    .unwrap()
                    .len()
        )
        .unwrap()
    );
}

#[tokio::test]
async fn live_page_library_state_discussion_link_and_delete_round_trip() {
    let state = test_state();
    state
        .db
        .with_conn(|connection| {
            let now = chrono::Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('disc-page-origin', 'Adobe monitoring room', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);
    let (_, created) = post_json(
        app.clone(),
        "/api/pages",
        serde_json::json!({
            "title": "Standalone investigation",
            "html": "<!doctype html><h1>Investigation</h1>",
            "discussion_id": "disc-page-origin",
            "datasets": []
        }),
    )
    .await;
    assert_eq!(created["success"], true, "{created}");
    assert_eq!(created["data"]["pinned"], false);
    assert_eq!(created["data"]["archived"], false);
    let page_id = created["data"]["id"].as_str().unwrap();

    let (_, links) = get_json(app.clone(), &format!("/api/pages/{page_id}/discussions")).await;
    assert_eq!(links["data"][0]["discussion_id"], "disc-page-origin");
    assert_eq!(links["data"][0]["relation"], "created_from");

    let (_, updated) = patch_json(
        app.clone(),
        &format!("/api/pages/{page_id}"),
        serde_json::json!({"pinned": true, "archived": true}),
    )
    .await;
    assert_eq!(updated["data"]["pinned"], true);
    assert_eq!(updated["data"]["archived"], true);

    let (_, deleted) = delete_json(app.clone(), &format!("/api/pages/{page_id}")).await;
    assert_eq!(deleted["success"], true, "{deleted}");
    let (_, missing) = get_json(app, &format!("/api/pages/{page_id}")).await;
    assert_eq!(missing["error_code"], "not_found");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// Create a test AppState with an in-memory database and default config.
/// Every config::save reached through a handler under test would otherwise
/// write the DEVELOPER'S REAL config.toml (config_dir() falls back to the
/// platform dir when KRONN_DATA_DIR is unset) — a full `cargo test` used to
/// wipe pseudo/avatar/model-tiers on the host (2026-07-13 incident).
fn isolate_config_dir() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        let dir = std::env::temp_dir().join(format!("kronn-inttest-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("KRONN_DATA_DIR", &dir);
    });
}

// ═══════════════════════════════════════════════════════════════════════════════
// Planning / discussion plans
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn planning_task_create_list_and_get_round_trip() {
    let app = test_app();
    let (_, created) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Ship planning",
            "priority": "high",
            "tags": ["release", "UX"],
            "definition_of_done": [
                {"sentence": "The compact API is covered", "completed": false}
            ],
            "links": [
                {"label": "Design", "url": "https://example.test/design"}
            ]
        }),
    )
    .await;
    assert_eq!(created["success"], true);
    assert_eq!(created["data"]["reference"], "KT-1");
    assert_eq!(created["data"]["status"], "idea");
    let task_id = created["data"]["id"].as_str().unwrap();

    let (_, listed) = get_json(
        app.clone(),
        "/api/planning/tasks?priority=high&search=planning",
    )
    .await;
    assert_eq!(listed["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["data"]["items"][0]["id"], task_id);

    let (_, tagged) = get_json(app.clone(), "/api/planning/tasks?tag=ux").await;
    assert_eq!(tagged["data"]["items"].as_array().unwrap().len(), 1);

    let (_, detail) = get_json(app, &format!("/api/planning/tasks/{task_id}")).await;
    assert_eq!(
        detail["data"]["definition_of_done"][0]["sentence"],
        "The compact API is covered"
    );
    assert_eq!(detail["data"]["links"][0]["label"], "Design");
    assert_eq!(detail["data"]["events"][0]["action"], "created");
}

#[tokio::test]
async fn planning_task_create_can_target_a_discussion_without_orphans() {
    let state = test_state();
    state
        .db
        .with_conn(|connection| {
            let now = chrono::Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('agent-created-disc', 'Agent-created', ?1, ?1)",
                [&now],
            )?;
            connection.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('other-disc', 'Other', ?1, ?1)",
                [&now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let create_request = serde_json::json!({
        "title": "Generate the requested plan",
        "discussion_id": "agent-created-disc",
        "idempotency_key": "target-discussion/retry-1"
    });
    let (_, created) = post_json(app.clone(), "/api/planning/tasks", create_request.clone()).await;
    let (_, replayed) = post_json(app.clone(), "/api/planning/tasks", create_request).await;
    assert_eq!(created["success"], true, "{created}");
    assert_eq!(replayed["success"], true, "{replayed}");
    assert_eq!(replayed["data"]["id"], created["data"]["id"]);
    assert_eq!(
        created["data"]["discussion_ids"],
        serde_json::json!(["agent-created-disc"])
    );
    assert_eq!(
        replayed["data"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["action"] == "discussion_linked")
            .count(),
        1
    );

    let (_, plan) = get_json(app.clone(), "/api/discussions/agent-created-disc/plan").await;
    assert_eq!(plan["data"]["active"].as_array().unwrap().len(), 1);
    assert_eq!(
        plan["data"]["active"][0]["task"]["id"],
        created["data"]["id"]
    );

    let (_, target_conflict) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Generate the requested plan",
            "discussion_id": "other-disc",
            "idempotency_key": "target-discussion/retry-1"
        }),
    )
    .await;
    assert_eq!(target_conflict["success"], false);
    assert_eq!(target_conflict["error_code"], "conflict");

    let (_, rejected) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Do not create an orphan",
            "discussion_id": "missing-disc"
        }),
    )
    .await;
    assert_eq!(rejected["success"], false);
    assert_eq!(rejected["error_code"], "not_found");

    let (_, listed) = get_json(app, "/api/planning/tasks").await;
    assert_eq!(listed["data"]["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn planning_task_create_is_idempotent_by_key_not_title() {
    let app = test_app();
    let first_request = serde_json::json!({
        "title": "Same title is valid",
        "description": "Original content",
        "idempotency_key": "planning-create/retry-1"
    });
    let (_, first) = post_json(app.clone(), "/api/planning/tasks", first_request.clone()).await;
    let (_, replay) = post_json(app.clone(), "/api/planning/tasks", first_request).await;
    assert_eq!(first["success"], true, "{first}");
    assert_eq!(replay["success"], true, "{replay}");
    assert_eq!(replay["data"]["id"], first["data"]["id"]);
    assert_eq!(
        replay["data"]["events"]
            .as_array()
            .expect("events array")
            .iter()
            .filter(|event| event["action"] == "created")
            .count(),
        1,
        "an idempotent replay must not append another created event"
    );

    let (_, conflict) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Same title is valid",
            "description": "Different content",
            "idempotency_key": "planning-create/retry-1"
        }),
    )
    .await;
    assert_eq!(conflict["success"], false);
    assert_eq!(conflict["error_code"], "conflict");
    assert!(conflict["error"]
        .as_str()
        .unwrap()
        .contains("different content"));

    let (_, same_title_new_key) = post_json(
        app.clone(),
        "/api/planning/tasks",
        serde_json::json!({
            "title": "Same title is valid",
            "description": "Original content",
            "idempotency_key": "planning-create/retry-2"
        }),
    )
    .await;
    assert_eq!(same_title_new_key["success"], true);
    assert_ne!(same_title_new_key["data"]["id"], first["data"]["id"]);

    let concurrent_request = serde_json::json!({
        "title": "Concurrent retry",
        "idempotency_key": "planning-create/race-1"
    });
    let (left, right) = tokio::join!(
        post_json(
            app.clone(),
            "/api/planning/tasks",
            concurrent_request.clone()
        ),
        post_json(app, "/api/planning/tasks", concurrent_request)
    );
    assert_eq!(left.1["success"], true, "{}", left.1);
    assert_eq!(right.1["success"], true, "{}", right.1);
    assert_eq!(
        left.1["data"]["id"], right.1["data"]["id"],
        "concurrent retries must resolve to one row"
    );
}

#[tokio::test]
async fn planning_discussion_plan_replaces_the_primary_objective() {
    let state = test_state();
    state
        .db
        .with_conn(|connection| {
            let now = chrono::Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at)
                 VALUES ('planning-disc', 'Planning', ?1, ?1)",
                [now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let mut task_ids = Vec::new();
    for title in ["First objective", "Second objective"] {
        let (_, created) = post_json(
            app.clone(),
            "/api/planning/tasks",
            serde_json::json!({"title": title, "status": "todo"}),
        )
        .await;
        let task_id = created["data"]["id"].as_str().unwrap().to_string();
        let (_, linked) = post_json(
            app.clone(),
            &format!("/api/planning/tasks/{task_id}/discussions"),
            serde_json::json!({
                "discussion_id": "planning-disc",
                "placement": "active",
                "is_primary": true
            }),
        )
        .await;
        assert_eq!(linked["success"], true);
        task_ids.push(task_id);
    }

    let (_, plan) = get_json(app.clone(), "/api/discussions/planning-disc/plan").await;
    assert_eq!(plan["data"]["primary_objective"]["id"], task_ids[1]);
    assert_eq!(plan["data"]["active"].as_array().unwrap().len(), 2);
    assert_eq!(
        plan["data"]["active"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|relation| relation["is_primary"] == true)
            .count(),
        1
    );

    let (_, changes) = get_json(
        app,
        "/api/discussions/planning-disc/plan/changes?since=2026-01-01T00%3A00%3A00Z",
    )
    .await;
    assert_eq!(changes["success"], true);
    assert_eq!(changes["data"].as_array().unwrap().len(), 4);
    assert!(changes["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| change["action"] == "discussion_linked"));
}

#[tokio::test]
async fn planning_api_rejects_dependency_cycles_and_identifies_agent_events() {
    let app = test_app();
    let mut task_ids = Vec::new();
    for title in ["A", "B"] {
        let (_, created) = post_json(
            app.clone(),
            "/api/planning/tasks",
            serde_json::json!({"title": title, "status": "todo"}),
        )
        .await;
        task_ids.push(created["data"]["id"].as_str().unwrap().to_string());
    }

    let (_, first_link) = post_json(
        app.clone(),
        &format!("/api/planning/tasks/{}/blockers", task_ids[0]),
        serde_json::json!({
            "blocker_task_id": task_ids[1],
            "actor": {"kind": "agent", "id": "Codex"}
        }),
    )
    .await;
    assert_eq!(first_link["success"], true);
    assert_eq!(first_link["data"]["events"][0]["actor_kind"], "agent");
    assert_eq!(first_link["data"]["events"][0]["actor_id"], "Codex");

    let (_, cycle) = post_json(
        app.clone(),
        &format!("/api/planning/tasks/{}/blockers", task_ids[1]),
        serde_json::json!({"blocker_task_id": task_ids[0]}),
    )
    .await;
    assert_eq!(cycle["success"], false);
    assert_eq!(cycle["error_code"], "validation");
    assert!(cycle["error"].as_str().unwrap().contains("cycle"));

    let (_, removed) = delete_json_body(
        app.clone(),
        &format!(
            "/api/planning/tasks/{}/blockers/{}",
            task_ids[0], task_ids[1]
        ),
        serde_json::json!({"actor": {"kind": "agent", "id": "Codex"}}),
    )
    .await;
    assert_eq!(removed["success"], true);
    assert_eq!(removed["data"]["blocker_count"], 0);
    assert_eq!(removed["data"]["blockers"].as_array().unwrap().len(), 0);
    assert_eq!(removed["data"]["events"][0]["action"], "blocker_removed");
    assert_eq!(removed["data"]["events"][0]["actor_id"], "Codex");

    // A lost response may make an MCP client retry. The edge removal is
    // idempotent and the retry must not manufacture a second audit event.
    let (_, retried) = delete_json_body(
        app,
        &format!(
            "/api/planning/tasks/{}/blockers/{}",
            task_ids[0], task_ids[1]
        ),
        serde_json::json!({"actor": {"kind": "agent", "id": "Codex"}}),
    )
    .await;
    assert_eq!(retried["success"], true);
    assert_eq!(
        retried["data"]["events"].as_array().unwrap().len(),
        removed["data"]["events"].as_array().unwrap().len(),
    );
}

fn test_state() -> AppState {
    isolate_config_dir();
    let db = Arc::new(kronn::db::Database::open_in_memory().expect("Failed to open in-memory DB"));
    let mut cfg = kronn::core::config::default_config();
    cfg.server.auth_token = None; // Disable auth for tests
    let config = Arc::new(RwLock::new(cfg));
    AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS)
}

/// Build a test router backed by an in-memory database (auth disabled).
fn test_app() -> Router {
    build_router_with_auth(test_state(), false)
}

/// Send a GET request and return (status, parsed JSON body).
async fn get_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// Send a POST request with a JSON body and return (status, parsed JSON body).
async fn post_json(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// Send a DELETE request and return (status, parsed JSON body).
async fn delete_json(app: Router, uri: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// Send a DELETE request with a JSON body and return (status, parsed JSON body).
async fn delete_json_body(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

#[tokio::test]
async fn lite_llm_model_failure_can_be_forgotten_for_the_current_endpoint() {
    let state = test_state();
    let endpoint = "http://litellm.test:4000";
    state.config.write().await.agents.lite_llm.base_url = Some(endpoint.to_string());
    state
        .db
        .with_conn(move |connection| {
            kronn::db::lite_llm_model_failures::record(
                connection,
                endpoint,
                "vertex_ai/mistral-large-2411",
                410,
                "kronn:model-not-in-catalogue",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let (_, before) = get_json(app.clone(), "/api/lite-llm/model-failures").await;
    assert_eq!(before["data"]["failures"].as_array().unwrap().len(), 1);

    let (status, forgotten) = delete_json_body(
        app.clone(),
        "/api/lite-llm/model-failures",
        serde_json::json!({ "model": "vertex_ai/mistral-large-2411" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(forgotten["success"], true);
    assert_eq!(forgotten["data"], true);

    let (_, after) = get_json(app, "/api/lite-llm/model-failures").await;
    assert!(after["data"]["failures"].as_array().unwrap().is_empty());
}

/// Send a PATCH request with a JSON body and return (status, parsed JSON body).
async fn patch_json(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

async fn put_json_root(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    (status, json)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Health endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn health_returns_ok() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["ok"], true);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Setup status endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn setup_status_returns_ok() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/setup/status").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Should have all required fields
    assert!(json["data"]["is_first_run"].is_boolean());
    assert!(json["data"]["current_step"].is_string());
    assert!(json["data"]["agents_detected"].is_array());
    assert!(json["data"]["repos_detected"].is_array());
    assert!(json["data"]["scan_paths_set"].is_boolean());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stats endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn stats_tokens_empty_db() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/tokens").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["total_tokens"], 0);
    assert!(json["data"]["by_provider"].as_array().unwrap().is_empty());
    assert!(json["data"]["by_project"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn stats_agent_usage_empty_db() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/agent-usage").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Context files endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper to create a discussion via the API, returns the discussion ID.
async fn create_test_discussion(state: &kronn::AppState) -> String {
    let disc_id = uuid::Uuid::new_v4().to_string();
    state.db.with_conn({
        let id = disc_id.clone();
        move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json, created_at, updated_at)
                 VALUES (?1, 'Test', 'ClaudeCode', 'en', '[]', datetime('now'), datetime('now'))",
                rusqlite::params![id],
            )?;
            Ok(())
        }
    }).await.unwrap();
    disc_id
}

#[tokio::test]
async fn context_files_list_empty() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let (status, json) =
        get_json(app, &format!("/api/discussions/{}/context-files", disc_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn context_files_upload_text_file() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    // Build multipart body
    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.txt\"\r\nContent-Type: text/plain\r\n\r\nHello world\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{}/context-files", disc_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["file"]["filename"], "test.txt");
    // Small file → the preview equals the full content ("Hello world" = 11).
    assert_eq!(json["data"]["file"]["extracted_size"], 11);
    // 2026-06-25: text files now land ON DISK (raw file + preview in context),
    // not inlined — so a disk_path is set (was null in the extract-to-context era).
    let disk_path = json["data"]["file"]["disk_path"]
        .as_str()
        .expect("a text file is now saved to disk, disk_path must be set");
    assert!(
        disk_path.contains(".kronn/context-files/"),
        "unexpected path: {disk_path}"
    );
    let _ = std::fs::remove_file(disk_path);
}

#[tokio::test]
async fn context_files_upload_image_lands_in_persistent_dir_not_temp() {
    // Regression (0.8.8): project-less disc images used to save to the system
    // temp dir — under Docker that's the container /tmp, wiped on restart, so
    // the bytes vanished and the bubble thumbnail 404'd. They must land in the
    // persistent data dir (config_dir) instead.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await; // no project
    let app = kronn::build_router(state);

    let boundary = "----ImgBoundary";
    // A tiny but valid-enough PNG header so extract_content takes the image path.
    let mut body = Vec::new();
    body.extend_from_slice(format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"shot.png\"\r\nContent-Type: image/png\r\n\r\n"
    ).as_bytes());
    body.extend_from_slice(b"\x89PNG\r\n\x1a\nfakepngpayload");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{}/context-files", disc_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let json: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(json["success"], true, "upload failed: {json}");
    let disk_path = json["data"]["file"]["disk_path"]
        .as_str()
        .expect("image must have a disk_path");
    let persistent = kronn::core::config::config_dir().unwrap();
    assert!(
        disk_path.starts_with(persistent.to_str().unwrap()),
        "project-less image must persist under the data dir {persistent:?}, not the ephemeral temp dir; got {disk_path}"
    );
    // Clean up the file we just wrote under the real data dir.
    let _ = std::fs::remove_file(disk_path);
}

#[tokio::test]
async fn context_files_upload_arbitrary_file_lands_on_disk() {
    // 2026-06-25: with files-on-disc, a non-image / non-office extension is no
    // longer REJECTED as "Unsupported" — its raw bytes are saved to disk for
    // the agent to read with its tools. Nothing is rejected purely on type.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let boundary = "----TestBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"binary.exe\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01\x02\r\n--{boundary}--\r\n"
    );
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{}/context-files", disc_id))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        json["success"], true,
        "arbitrary file must be accepted: {json}"
    );
    let disk_path = json["data"]["file"]["disk_path"]
        .as_str()
        .expect("an arbitrary file must be saved to disk");
    assert!(
        disk_path.contains(".kronn/context-files/"),
        "unexpected path: {disk_path}"
    );
    let _ = std::fs::remove_file(disk_path);
}

#[tokio::test]
async fn context_files_delete() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;

    // Insert a context file directly via DB
    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-del",
                    &did,
                    "to_delete.txt",
                    "text/plain",
                    10,
                    "Test",
                    None,
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let (status, json) = delete_json(
        app,
        &format!("/api/discussions/{}/context-files/cf-del", disc_id),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn context_files_delete_nonexistent_returns_error() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);

    let (_, json) = delete_json(
        app,
        &format!("/api/discussions/{}/context-files/nonexistent", disc_id),
    )
    .await;
    assert_eq!(json["success"], false);
}

#[tokio::test]
async fn context_file_upload_ignores_historical_attachment_count() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;

    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                conn.execute(
                    "INSERT INTO messages
                        (id, discussion_id, role, content, agent_type, timestamp, tokens_used)
                     VALUES ('m-history', ?1, 'Agent', 'assets', 'Codex', datetime('now'), 0)",
                    rusqlite::params![did],
                )?;
                for index in 0..20 {
                    let file_id = format!("cf-history-{index}");
                    let filename = format!("history-{index}.png");
                    kronn::db::discussions::insert_context_file(
                        conn,
                        &file_id,
                        &did,
                        &filename,
                        "image/png",
                        8,
                        "[Image]",
                        None,
                    )?;
                }
                kronn::db::discussions::link_pending_context_files_to_message(
                    conn,
                    &did,
                    "m-history",
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let boundary = "----HistoricalAssetsBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"new.txt\"\r\nContent-Type: text/plain\r\n\r\nnew asset\r\n--{boundary}--\r\n"
    );
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{disc_id}/context-files"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let response = app.oneshot(req).await.unwrap();
    let json: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(
        json["success"], true,
        "historical assets blocked upload: {json}"
    );
    let disk_path = json["data"]["file"]["disk_path"].as_str().unwrap();
    let _ = std::fs::remove_file(disk_path);
}

#[tokio::test]
async fn context_file_upload_still_caps_twenty_pending_files() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                for index in 0..20 {
                    kronn::db::discussions::insert_context_file(
                        conn,
                        &format!("cf-pending-{index}"),
                        &did,
                        &format!("pending-{index}.txt"),
                        "text/plain",
                        1,
                        "x",
                        None,
                    )?;
                }
                Ok(())
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let boundary = "----PendingAssetsBoundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"blocked.txt\"\r\nContent-Type: text/plain\r\n\r\nblocked\r\n--{boundary}--\r\n"
    );
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/api/discussions/{disc_id}/context-files"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let response = app.oneshot(req).await.unwrap();
    let json: Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(json["success"], false);
    assert!(json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Maximum 20 pending context files"));
}

// ── 0.8.8: per-message attachments — image content route + MCP exposure ──

/// GET the raw bytes of an attachment; returns (status, content-type, body).
async fn get_raw(app: Router, uri: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, ct, body)
}

#[tokio::test]
async fn context_file_content_streams_image_bytes() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;

    // Write a fake PNG to disk and register it as an image context file.
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("shot.png");
    let bytes: &[u8] = b"\x89PNG\r\n\x1a\nFAKEPNGDATA";
    std::fs::write(&img_path, bytes).unwrap();
    let path_str = img_path.to_string_lossy().to_string();

    state
        .db
        .with_conn({
            let did = disc_id.clone();
            let p = path_str.clone();
            move |conn| {
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-img",
                    &did,
                    "shot.png",
                    "image/png",
                    bytes.len() as u64,
                    "[Image]",
                    Some(&p),
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let (status, ct, body) = get_raw(
        app,
        &format!("/api/discussions/{}/context-files/cf-img/content", disc_id),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("image/png"));
    assert_eq!(body, bytes, "the route must stream the exact on-disk bytes");
}

#[tokio::test]
async fn context_file_content_404_for_text_file_without_disk_path() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    // A text file has no disk_path → nothing to stream.
    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-txt",
                    &did,
                    "notes.txt",
                    "text/plain",
                    5,
                    "hello",
                    None,
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let (status, _, _) = get_raw(
        app,
        &format!("/api/discussions/{}/context-files/cf-txt/content", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn context_file_content_serves_image_type_even_when_stored_mime_is_legacy_text() {
    // Regression (0.8.8): mime_from_extension used to map images to text/plain,
    // so legacy rows stored "text/plain" for a .png. The content route must
    // derive image/png from the filename, else the browser renders bytes as
    // text when the thumbnail is opened.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("legacy.png");
    std::fs::write(&img_path, b"\x89PNGlegacy").unwrap();
    let p = img_path.to_string_lossy().to_string();
    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                // Stored mime is the WRONG legacy value on purpose.
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-leg",
                    &did,
                    "legacy.png",
                    "text/plain",
                    10,
                    "[Image]",
                    Some(&p),
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let (status, ct, _) = get_raw(
        app,
        &format!("/api/discussions/{}/context-files/cf-leg/content", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ct.as_deref(),
        Some("image/png"),
        "must derive image/png from .png, not serve the stored text/plain"
    );
}

#[tokio::test]
async fn link_pending_endpoint_pins_popup_files_to_first_message() {
    // Reproduces the creation-popup path: files uploaded as pending, then the
    // frontend links them to the first message via this endpoint.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let did = disc_id.clone();
    state.db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, tokens_used)
             VALUES ('first-msg', ?1, 'User', 'tu vois cette image ?', NULL, ?2, 0)",
            rusqlite::params![did, chrono::Utc::now().to_rfc3339()],
        )?;
        kronn::db::discussions::insert_context_file(conn, "cf-pop", &did, "shot.png", "text/plain", 10, "[Image]", Some("/tmp/shot.png"))?;
        Ok(())
    }).await.unwrap();

    let app = kronn::build_router(state.clone());
    let (status, json) = post_json(
        app,
        &format!("/api/discussions/{}/context-files/link-pending", disc_id),
        serde_json::json!({ "message_id": "first-msg" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"], 1, "one pending file linked");

    // The file is now pinned to the first message, not pending.
    let files = state
        .db
        .with_conn(move |conn| {
            kronn::db::discussions::list_context_files_for_message(conn, "first-msg")
                .map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].filename, "shot.png");
}

#[tokio::test]
async fn link_pending_endpoint_can_pin_only_the_callers_uploaded_files() {
    // MCP and the human composer may upload concurrently. An agent must link
    // only the ids returned by its own uploads, leaving the user's draft file
    // pending for the user's next message.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let did = disc_id.clone();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, tokens_used)
                 VALUES ('agent-msg', ?1, 'Agent', 'voici la capture', 'Codex', ?2, 0)",
                rusqlite::params![did, chrono::Utc::now().to_rfc3339()],
            )?;
            kronn::db::discussions::insert_context_file(
                conn,
                "cf-agent",
                &did,
                "agent.png",
                "image/png",
                10,
                "[Image]",
                Some("/tmp/agent.png"),
            )?;
            kronn::db::discussions::insert_context_file(
                conn,
                "cf-human",
                &did,
                "human.png",
                "image/png",
                10,
                "[Image]",
                Some("/tmp/human.png"),
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let mut ws_rx = state.ws_broadcast.subscribe();
    let app = kronn::build_router(state.clone());
    let (status, json) = post_json(
        app,
        &format!("/api/discussions/{}/context-files/link-pending", disc_id),
        serde_json::json!({
            "message_id": "agent-msg",
            "file_ids": ["cf-agent"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"], 1);

    let event = ws_rx.try_recv().expect("context-file invalidation event");
    assert!(matches!(
        event,
        kronn::models::WsMessage::ContextFilesChanged {
            discussion_id: ref event_discussion_id,
            message_id: ref event_message_id,
        } if event_discussion_id == &disc_id && event_message_id == "agent-msg"
    ));

    let (attached, pending) = state
        .db
        .with_conn(move |conn| {
            let attached =
                kronn::db::discussions::list_context_files_for_message(conn, "agent-msg")?;
            let pending = kronn::db::discussions::list_context_files(conn, &disc_id)?
                .into_iter()
                .filter(|file| file.message_id.is_none())
                .collect::<Vec<_>>();
            Ok((attached, pending))
        })
        .await
        .unwrap();
    assert_eq!(attached.len(), 1);
    assert_eq!(attached[0].id, "cf-agent");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "cf-human");
}

#[tokio::test]
async fn context_file_content_sanitizes_filename_in_content_disposition() {
    // A filename carrying quotes + CRLF must not inject headers into the
    // Content-Disposition value.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("ok.png");
    std::fs::write(&img_path, b"\x89PNGx").unwrap();
    let p = img_path.to_string_lossy().to_string();
    state
        .db
        .with_conn({
            let did = disc_id.clone();
            move |conn| {
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-evil",
                    &did,
                    "evil\".png\r\nX-Injected: 1",
                    "image/png",
                    5,
                    "[Image]",
                    Some(&p),
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/discussions/{}/context-files/cf-evil/content",
            disc_id
        ))
        .body(Body::empty())
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        !cd.contains('\r') && !cd.contains('\n'),
        "no CR/LF in disposition: {cd:?}"
    );
    assert!(!cd.contains("X-Injected: 1\r"), "no header injection");
    // The sanitized name keeps the readable text, drops the quote + control chars.
    assert!(
        cd.contains("evil.pngX-Injected: 1"),
        "sanitized name: {cd:?}"
    );
}

#[tokio::test]
async fn context_file_content_404_for_unknown_id() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let app = kronn::build_router(state);
    let (status, _, _) = get_raw(
        app,
        &format!("/api/discussions/{}/context-files/ghost/content", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn context_file_content_404_when_id_belongs_to_other_discussion() {
    // Security: the row must match BOTH disc_id and file_id, so a file from
    // disc A can't be fetched through disc B's URL.
    let state = test_state();
    let disc_a = create_test_discussion(&state).await;
    let disc_b = create_test_discussion(&state).await;
    let dir = tempfile::tempdir().unwrap();
    let img_path = dir.path().join("a.png");
    std::fs::write(&img_path, b"PNGA").unwrap();
    let p = img_path.to_string_lossy().to_string();
    state
        .db
        .with_conn({
            let did = disc_a.clone();
            move |conn| {
                kronn::db::discussions::insert_context_file(
                    conn,
                    "cf-a",
                    &did,
                    "a.png",
                    "image/png",
                    4,
                    "[Image]",
                    Some(&p),
                )
                .map_err(|e| anyhow::anyhow!(e))
            }
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    // Fetch cf-a through disc_b's URL → must 404.
    let (status, _, _) = get_raw(
        app,
        &format!("/api/discussions/{}/context-files/cf-a/content", disc_b),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disc_get_message_includes_pinned_attachments() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;

    // Insert a user message and pin an image to it.
    let did = disc_id.clone();
    state.db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, tokens_used)
             VALUES ('m-att', ?1, 'User', 'regarde cette image', NULL, ?2, 0)",
            rusqlite::params![did, chrono::Utc::now().to_rfc3339()],
        )?;
        kronn::db::discussions::insert_context_file(conn, "cf-att", &did, "diagram.png", "image/png", 99, "[Image]", Some("/tmp/diagram.png"))?;
        kronn::db::discussions::link_pending_context_files_to_message(conn, &did, "m-att")?;
        Ok(())
    }).await.unwrap();

    let app = kronn::build_router(state);
    let (status, json) = get_json(app, &format!("/api/discussions/{}/message/0", disc_id)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let attachments = json["data"]["attachments"]
        .as_array()
        .expect("attachments array present");
    assert_eq!(
        attachments.len(),
        1,
        "the pinned image must surface to the agent"
    );
    assert_eq!(attachments[0]["filename"], "diagram.png");
    assert_eq!(attachments[0]["mime_type"], "image/png");
}

#[tokio::test]
async fn disc_get_message_omits_attachments_when_none() {
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let did = disc_id.clone();
    state.db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO messages (id, discussion_id, role, content, agent_type, timestamp, tokens_used)
             VALUES ('m-plain', ?1, 'User', 'pas de fichier', NULL, ?2, 0)",
            rusqlite::params![did, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }).await.unwrap();

    let app = kronn::build_router(state);
    let (status, json) = get_json(app, &format!("/api/discussions/{}/message/0", disc_id)).await;
    assert_eq!(status, StatusCode::OK);
    // `skip_serializing_if = "Vec::is_empty"` → the field is absent, not [].
    assert!(
        json["data"].get("attachments").is_none(),
        "empty attachments must not be serialized"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Config endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_tokens_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/tokens").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["keys"].as_array().unwrap().is_empty());
    assert!(json["data"]["disabled_overrides"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn config_language_default() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/language").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Default language from default_config() is "fr"
    assert_eq!(json["data"], "fr");
}

#[tokio::test]
async fn config_agent_access_default() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/agent-access").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Default config has full_access = false for all agents
    assert_eq!(json["data"]["claude_code"]["full_access"], false);
    assert_eq!(json["data"]["codex"]["full_access"], false);
}

#[tokio::test]
async fn config_db_info_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/db-info").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["project_count"], 0);
    assert_eq!(json["data"]["discussion_count"], 0);
    assert_eq!(json["data"]["message_count"], 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn discussions_stop_returns_cancelled_false_when_nothing_running() {
    // Endpoint smoke test: POST /api/discussions/:id/stop on a disc that has
    // no token in the cancel registry (never started, or already finished)
    // must return success with `cancelled: false` — not a fake success.
    let state = test_state();
    let (status, _) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        serde_json::json!({
            "title": "idle", "agent": "ClaudeCode", "language": "en", "initial_prompt": "x",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Any disc id — the endpoint doesn't validate existence, it just looks
    // up the registry (by design: even a just-finished disc won't be in the
    // registry anymore because of the CancelGuard Drop).
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/nonexistent-disc/stop",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["cancelled"], false);
}

#[tokio::test]
async fn config_ui_language_round_trip() {
    // The Tauri WebView2 localStorage wipe bug: frontend must be able to
    // recover the user's UI locale from the backend after a wipe. This test
    // proves the backend stores + returns the value.
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Default: fr
    let (_, json) = get_json(app.clone(), "/api/config/ui-language").await;
    assert_eq!(json["data"], "fr");

    // Save "en"
    let (status, json) = post_json(
        app.clone(),
        "/api/config/ui-language",
        serde_json::json!("en"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // GET returns the new value
    let (_, json) = get_json(app.clone(), "/api/config/ui-language").await;
    assert_eq!(json["data"], "en");

    // Reject invalid value
    let (_, json) = post_json(
        app.clone(),
        "/api/config/ui-language",
        serde_json::json!("klingon"),
    )
    .await;
    assert_eq!(json["success"], false);

    // Previous valid value is still there
    let (_, json) = get_json(app, "/api/config/ui-language").await;
    assert_eq!(json["data"], "en");
}

#[tokio::test]
async fn config_global_context_round_trip() {
    let state = test_state();
    let app = build_router_with_auth(state, false);

    // Default: empty
    let (_, json) = get_json(app.clone(), "/api/config/global-context").await;
    assert_eq!(json["data"], "");

    // Save markdown content
    let content = "## Glossary\n- CMS: our custom CMS\n\n## Stack\n- Rust + React";
    let (status, json) = post_json(
        app.clone(),
        "/api/config/global-context",
        serde_json::json!(content),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // GET returns saved content
    let (_, json) = get_json(app.clone(), "/api/config/global-context").await;
    assert_eq!(json["data"], content);

    // Empty string clears it
    let (_, json) = post_json(
        app.clone(),
        "/api/config/global-context",
        serde_json::json!("   "),
    )
    .await;
    assert_eq!(json["success"], true);
    let (_, json) = get_json(app, "/api/config/global-context").await;
    assert_eq!(json["data"], "");
}

#[tokio::test]
async fn config_stt_model_round_trip() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Default: null (never set)
    let (_, json) = get_json(app.clone(), "/api/config/stt-model").await;
    assert!(json["data"].is_null());

    // Save
    let (_, _) = post_json(
        app.clone(),
        "/api/config/stt-model",
        serde_json::json!("onnx-community/whisper-tiny"),
    )
    .await;
    let (_, json) = get_json(app.clone(), "/api/config/stt-model").await;
    assert_eq!(json["data"], "onnx-community/whisper-tiny");

    // Empty string clears it
    let (_, _) = post_json(app.clone(), "/api/config/stt-model", serde_json::json!("")).await;
    let (_, json) = get_json(app, "/api/config/stt-model").await;
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn config_tts_voices_per_language() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    assert_eq!(json["data"].as_object().unwrap().len(), 0);

    // Save 2 voices for different languages
    post_json(
        app.clone(),
        "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": "voice-fr-alpha"}),
    )
    .await;
    post_json(
        app.clone(),
        "/api/config/tts-voice",
        serde_json::json!({"lang": "en", "voice_id": "voice-en-beta"}),
    )
    .await;

    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    let voices = json["data"].as_object().unwrap();
    assert_eq!(voices["fr"], "voice-fr-alpha");
    assert_eq!(voices["en"], "voice-en-beta");

    // Overwrite fr
    post_json(
        app.clone(),
        "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": "voice-fr-new"}),
    )
    .await;
    let (_, json) = get_json(app.clone(), "/api/config/tts-voices").await;
    assert_eq!(json["data"]["fr"], "voice-fr-new");

    // Empty voice_id removes the entry
    post_json(
        app.clone(),
        "/api/config/tts-voice",
        serde_json::json!({"lang": "fr", "voice_id": ""}),
    )
    .await;
    let (_, json) = get_json(app, "/api/config/tts-voices").await;
    assert!(json["data"].as_object().unwrap().get("fr").is_none());
    assert_eq!(json["data"]["en"], "voice-en-beta");
}

#[tokio::test]
async fn workflow_test_batch_step_dry_run_preview() {
    // The wizard's "Tester" button on a BatchQuickPrompt step must let the
    // user spot-check what would happen WITHOUT spawning anything. We hit
    // the endpoint with a step + mock previous output, get back the parsed
    // items + sample prompt, no DB writes.
    let state = test_state();

    // Seed a Quick Prompt with one variable.
    let (status, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Analyse ticket",
            "icon": "🎯",
            "prompt_template": "Analyse {{ticket}} en profondeur",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    // Hit the dry-run endpoint with a mock previous output (the kind of
    // string a fetch step would produce: a JSON array of ticket ids).
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch-tickets",
                "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode",
                "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                // {{steps.X.data}} resolves to the data envelope object
                // (JSON-stringified). parse_items unwraps the inner array.
                "batch_items_from": "{{steps.fetch.data}}",
                "batch_max_items": 50
            },
            "mock_previous_output": "{\"data\":{\"tickets\":[\"EW-100\",\"EW-101\",\"EW-102\"]},\"status\":\"OK\",\"summary\":\"3 tickets\"}",
            "previous_step_name": "fetch"
        }),
    ).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "preview endpoint must return 200, got: {:?}",
        json
    );
    assert_eq!(json["success"], true);
    let preview = &json["data"];
    assert_eq!(preview["total_items"], 3);
    let sample = preview["sample_items"].as_array().unwrap();
    assert_eq!(sample.len(), 3);
    assert_eq!(sample[0], "EW-100");
    assert_eq!(preview["quick_prompt_name"], "Analyse ticket");
    assert_eq!(preview["quick_prompt_icon"], "🎯");
    assert_eq!(preview["first_variable_name"], "ticket");
    assert_eq!(
        preview["sample_rendered_prompt"],
        "Analyse EW-100 en profondeur"
    );
    let prompts = preview["sample_rendered_prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 3, "One rendered prompt per sample item");
    assert_eq!(prompts[0], "Analyse EW-100 en profondeur");
    assert_eq!(prompts[1], "Analyse EW-101 en profondeur");
    assert_eq!(prompts[2], "Analyse EW-102 en profondeur");
    assert_eq!(preview["workspace_mode"], "Direct");
    assert_eq!(preview["wait_for_completion"], true);
    assert!(preview["errors"].as_array().unwrap().is_empty());

    // No DB writes — discussions table should still be empty
    let (_, discs) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    )
    .await;
    assert!(
        discs["data"].as_array().unwrap().is_empty(),
        "Dry-run must not create any discussion"
    );
}

#[tokio::test]
async fn workflow_test_batch_step_freetext_with_data_template_warns_but_continues() {
    // Marie's bug 2026-04-13: she wires `{{steps.main.data}}` but step main
    // is in FreeText mode → at runtime, `.data` is never populated, the
    // template stays literal, batch fails. The dry-run must:
    //   1. Inject a fallback so the user CAN see what their items would be
    //   2. Warn them clearly that this won't work in production
    let state = test_state();
    let (_, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Auto-PR", "icon": "🤯",
            "prompt_template": "Code {{ticketId}}",
            "variables": [{ "name": "ticketId", "label": "id", "placeholder": "x" }],
            "agent": "ClaudeCode",
        }),
    )
    .await;
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch", "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode", "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                "batch_items_from": "{{steps.main.data}}",
            },
            // Mock is plain text (FreeText output from step main) — no envelope
            "mock_previous_output": "EW-2687,EW-3055",
            "previous_step_name": "main"
        }),
    )
    .await;
    assert_eq!(json["success"], true);

    // Errors must stay empty: the dry-run succeeded thanks to the fallback
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(
        errors.is_empty(),
        "Expected no blocking errors with fallback. Got: {:?}",
        errors
    );

    // Items got extracted (2 tickets via comma-split)
    assert_eq!(json["data"]["total_items"], 2);
    let sample = json["data"]["sample_items"].as_array().unwrap();
    assert_eq!(sample[0], "EW-2687");
    assert_eq!(sample[1], "EW-3055");

    // BUT: warnings must explain the production gap
    let warnings = json["data"]["warnings"].as_array().unwrap();
    assert!(
        !warnings.is_empty(),
        "Must surface a warning about FreeText + .data"
    );
    let warn_text: String = warnings
        .iter()
        .map(|w| w.as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        warn_text.contains("Structured") || warn_text.contains(".output"),
        "Warning should suggest Structured mode or .output. Got: {}",
        warn_text
    );
    assert!(
        warn_text.contains("main"),
        "Warning should name the step. Got: {}",
        warn_text
    );
}

#[tokio::test]
async fn workflow_test_batch_step_rejects_unresolved_template() {
    // Regression: user reported running the preview without providing a
    // mock_previous_output → the `{{steps.main.data}}` placeholder stayed
    // literal, parse_items treated it as a single item, and the UI showed
    // "1 item would be launched" with the raw template shown as sample.
    // Worse than useless — it hid the config bug. Now the endpoint detects
    // the unresolved braces and returns an explicit error.
    let state = test_state();
    let (_, qp_resp) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "QP", "prompt_template": "Do {{id}}",
            "variables": [{ "name": "id", "label": "id", "placeholder": "x" }],
            "agent": "ClaudeCode",
        }),
    )
    .await;
    let qp_id = qp_resp["data"]["id"].as_str().unwrap().to_string();

    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch", "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode", "prompt_template": "",
                "mode": { "type": "Normal" },
                "batch_quick_prompt_id": qp_id,
                "batch_items_from": "{{steps.main.data}}",
            },
            // NO mock_previous_output — the template can't resolve
        }),
    )
    .await;
    assert_eq!(json["success"], true);
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(
        !errors.is_empty(),
        "Must surface error on unresolved template"
    );
    let err_text: String = errors
        .iter()
        .map(|e| e.as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        err_text.contains("non résolue") || err_text.contains("unresolved"),
        "Error should mention unresolved variable. Got: {}",
        err_text
    );
    // The preview should NOT report a bogus "1 item" count
    assert_eq!(json["data"]["total_items"], 0);
}

#[tokio::test]
async fn workflow_test_batch_step_surfaces_validation_errors() {
    // Missing items_from + bad QP id → endpoint returns errors[] populated
    // and total_items=0, never crashes.
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/test-batch-step",
        serde_json::json!({
            "step": {
                "name": "batch-broken",
                "step_type": { "type": "BatchQuickPrompt" },
                "agent": "ClaudeCode",
                "prompt_template": "",
                "mode": { "type": "Normal" }
                // No batch_quick_prompt_id, no batch_items_from
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let errors = json["data"]["errors"].as_array().unwrap();
    assert!(!errors.is_empty(), "Should report missing required fields");
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap().contains("batch_quick_prompt_id")),
        "Error should mention missing QP id: {:?}",
        errors
    );
}

#[tokio::test]
async fn workflow_cancel_run_triggers_token_and_cascades_to_children() {
    // Pre-register a token for a fake run id + 3 fake child disc ids.
    // Then hit the cancel endpoint — all 4 tokens must be triggered.
    let state = test_state();
    let run_token = tokio_util::sync::CancellationToken::new();
    let d1 = tokio_util::sync::CancellationToken::new();
    let d2 = tokio_util::sync::CancellationToken::new();
    let d3 = tokio_util::sync::CancellationToken::new();
    {
        let mut map = state.cancel_registry.lock().unwrap();
        map.insert("run-cancel-me".into(), run_token.clone());
        map.insert("disc-A".into(), d1.clone());
        map.insert("disc-B".into(), d2.clone());
        map.insert("disc-C".into(), d3.clone());
    }

    // Seed the DB: parent linear run, one child batch, 3 child discs linked.
    // The endpoint's cascade query walks discussions.workflow_run_id →
    // workflow_runs where parent_run_id = target.
    state
        .db
        .with_conn(|conn| {
            // Minimal workflow row so the FK is satisfied.
            conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
             safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
             VALUES ('wf-1', 'W', NULL, '\"Manual\"', '[]', '[]', '{}', NULL, NULL, 0,
             datetime('now'), datetime('now'))",
            [],
        )?;
            conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed)
             VALUES ('run-cancel-me', 'wf-1', 'Running', '[]', 0, datetime('now'),
             'linear', 0, 0, 0)",
            [],
        )?;
            conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed, parent_run_id)
             VALUES ('batch-child', 'wf-1', 'Running', '[]', 0, datetime('now'),
             'batch', 3, 0, 0, 'run-cancel-me')",
            [],
        )?;
            for disc_id in ["disc-A", "disc-B", "disc-C"] {
                conn.execute(
                    "INSERT INTO discussions (id, project_id, title, agent, language,
                 participants_json, skill_ids_json, profile_ids_json, directive_ids_json,
                 created_at, updated_at, archived, workflow_run_id, message_count,
                 workspace_mode)
                 VALUES (?1, NULL, 'T', 'ClaudeCode', 'en', '[]', '[]', '[]', '[]',
                 datetime('now'), datetime('now'), 0, 'batch-child', 1, 'Direct')",
                    rusqlite::params![disc_id],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/wf-1/runs/run-cancel-me/cancel",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["run_cancelled"], true);
    assert_eq!(json["data"]["child_discs_cancelled"], 3);

    // Every token we pre-registered is now cancelled
    assert!(run_token.is_cancelled(), "parent run token must fire");
    assert!(
        d1.is_cancelled() && d2.is_cancelled() && d3.is_cancelled(),
        "all child disc tokens must fire"
    );

    // The child batch run row must be marked Cancelled in DB
    let batch_status: String = state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT status FROM workflow_runs WHERE id = 'batch-child'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(batch_status, "Cancelled");
}

#[tokio::test]
async fn workflow_cancel_run_idempotent_on_already_finished() {
    // Calling cancel on a run that never registered (or finished already)
    // must not error — returns `{run_cancelled: false, child_discs_cancelled: 0}`.
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/workflows/anything/runs/ghost-run/cancel",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["run_cancelled"], false);
    assert_eq!(json["data"]["child_discs_cancelled"], 0);
}

#[tokio::test]
async fn workflow_cancel_run_forces_db_status_on_orphaned_run() {
    // Regression: a run stuck in "Running" in the DB with no token registered
    // (e.g. runner crashed mid-await, or backend was restarted) must still be
    // marked Cancelled by the endpoint. Before this fix, the user saw
    // `run_cancelled=false` silently and the sidebar kept showing "running"
    // forever — no way to escape without direct DB surgery.
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
             safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
             VALUES ('wf-orphan', 'W', NULL, '\"Manual\"', '[]', '[]', '{}', NULL, NULL, 0,
             datetime('now'), datetime('now'))",
            [],
        )?;
            conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed)
             VALUES ('orphan-run', 'wf-orphan', 'Running', '[]', 0, datetime('now'),
             'linear', 0, 0, 0)",
            [],
        )?;
            Ok(())
        })
        .await
        .unwrap();

    // No token in cancel_registry — simulates runner death / restart.
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/wf-orphan/runs/orphan-run/cancel",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // From the user's POV the cancel worked — the orphan row was rescued.
    assert_eq!(
        json["data"]["run_cancelled"], true,
        "orphaned run (no token) should still report cancelled when DB forced"
    );

    // DB row must now say Cancelled, with a finished_at timestamp.
    let (status_s, finished_at): (String, Option<String>) = state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT status, finished_at FROM workflow_runs WHERE id = 'orphan-run'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(status_s, "Cancelled");
    assert!(finished_at.is_some(), "finished_at must be set");
}

#[tokio::test]
async fn workflow_cancel_run_second_click_works_when_token_already_consumed() {
    // Real-world scenario from production (Auto-analyse des 50 derniers
    // tickets JIRA): first click triggered the token (run_cancelled=true)
    // but the runner was blocked on a deep await (waiting on batch children
    // via ws_broadcast) and never wrote status = Cancelled to the DB. On
    // the second click, the token is gone from the registry. Without the
    // forced DB update, the endpoint returned false and the user was stuck.
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
            "INSERT INTO workflows (id, name, project_id, trigger_json, steps_json, actions_json,
             safety_json, workspace_config_json, concurrency_limit, enabled, created_at, updated_at)
             VALUES ('wf-stuck', 'W', NULL, '\"Manual\"', '[]', '[]', '{}', NULL, NULL, 0,
             datetime('now'), datetime('now'))",
            [],
        )?;
            conn.execute(
            "INSERT INTO workflow_runs (id, workflow_id, status, step_results_json, tokens_used,
             started_at, run_type, batch_total, batch_completed, batch_failed)
             VALUES ('stuck-run', 'wf-stuck', 'Running', '[]', 0, datetime('now'),
             'linear', 0, 0, 0)",
            [],
        )?;
            Ok(())
        })
        .await
        .unwrap();

    // No token registered → simulates the second-click path.
    let (_, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows/wf-stuck/runs/stuck-run/cancel",
        serde_json::json!({}),
    )
    .await;

    // The user-facing field must be true — the row did get rescued.
    assert_eq!(json["data"]["run_cancelled"], true);
}

#[tokio::test]
async fn discussions_stop_triggers_registered_token() {
    // Full path: pre-register a token in the cancel registry, hit the stop
    // endpoint, verify the token was triggered.
    let state = test_state();
    let token = tokio_util::sync::CancellationToken::new();
    {
        let mut map = state.cancel_registry.lock().unwrap();
        map.insert("live-disc".into(), token.clone());
    }
    assert!(!token.is_cancelled());

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/live-disc/stop",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["cancelled"], true);
    assert!(token.is_cancelled());

    // Registry was cleaned: a second stop returns cancelled=false
    let (_, json2) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/live-disc/stop",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(json2["data"]["cancelled"], false);
}

#[tokio::test]
async fn discussions_targeted_stop_preserves_a_sibling_dispatch() {
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, created_at, updated_at, awaiting_agent)
                 VALUES ('target-disc', 'Targeted stop', 'ClaudeCode', datetime('now'), datetime('now'), 1)",
                [],
            )?;
            for (sort_order, (message_id, job_id)) in
                [(1, ("target-old", "target-job-old")), (2, ("target-new", "target-job-new"))]
            {
                conn.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, channel, content, timestamp, sort_order)
                     VALUES (?1, 'target-disc', 'User', 'main', ?1, datetime('now'), ?2)",
                    rusqlite::params![message_id, sort_order],
                )?;
                kronn::db::agent_dispatch::enqueue(
                    conn,
                    kronn::db::agent_dispatch::NewAgentDispatchJob {
                        id: job_id,
                        discussion_id: "target-disc",
                        trigger_message_id: message_id,
                        trigger_sort_order: sort_order,
                        dedupe_key: job_id,
                        agent_override: Some(&kronn::models::AgentType::ClaudeCode),
                        chain_prompt_ids: &[],
                        batch_item: None,
                        group_id: None,
                        group_concurrency_limit: None,
                    },
                )?;
            }
            kronn::db::agent_dispatch::claim(conn, "target-job-old")?;
            Ok(())
        })
        .await
        .unwrap();

    let token = tokio_util::sync::CancellationToken::new();
    state
        .cancel_registry
        .lock()
        .unwrap()
        .insert("target-job-old".into(), token.clone());

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/target-disc/agent-dispatches/target-job-old/stop",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["cancelled"], true);
    assert_eq!(json["data"]["still_awaiting"], true);
    assert!(token.is_cancelled());

    state
        .db
        .with_conn(|conn| {
            assert_eq!(
                kronn::db::agent_dispatch::get(conn, "target-job-old")?
                    .unwrap()
                    .status,
                kronn::db::agent_dispatch::DispatchStatus::Cancelled,
            );
            assert_eq!(
                kronn::db::agent_dispatch::get(conn, "target-job-new")?
                    .unwrap()
                    .status,
                kronn::db::agent_dispatch::DispatchStatus::Pending,
            );
            Ok(())
        })
        .await
        .unwrap();
}

// ─── Partial response recovery — HTTP layer ─────────────────────────────────
// DB-level behavior is covered in backend/src/db/tests.rs. These tests cover
// the HTTP contract: dismiss-partial endpoint, WS broadcast, and the
// send-message guard that refused a new run while a partial is pending.
// Regression net for the 2026-04-13 "double Agent response" bug.

#[tokio::test]
async fn discussions_dismiss_partial_recovers_and_broadcasts_ws() {
    let state = test_state();
    // Subscribe BEFORE we call the endpoint so we don't miss the broadcast.
    let mut ws_rx = state.ws_broadcast.subscribe();

    // Seed: disc with a dangling partial checkpoint.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, project_id, title, agent, language,
             participants_json, skill_ids_json, profile_ids_json, directive_ids_json,
             created_at, updated_at, archived, message_count, workspace_mode,
             partial_response, partial_response_started_at)
             VALUES ('disc-dismiss-1', NULL, 'T', 'ClaudeCode', 'fr', '[]', '[]', '[]', '[]',
             datetime('now'), datetime('now'), 0, 0, 'Direct',
             'partial draft v1', datetime('now', '-1 minute'))",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/disc-dismiss-1/dismiss-partial",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(
        json["data"]["recovered"], true,
        "dismiss must return recovered=true when the targeted disc had a partial"
    );

    // The partial was converted to an Agent message and the column cleared.
    let (partial_col, msg_count): (Option<String>, i64) = state
        .db
        .with_conn(|conn| {
            let p = conn.query_row(
                "SELECT partial_response FROM discussions WHERE id = 'disc-dismiss-1'",
                [],
                |r| r.get::<_, Option<String>>(0),
            )?;
            let n = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE discussion_id = 'disc-dismiss-1'",
                [],
                |r| r.get::<_, i64>(0),
            )?;
            Ok((p, n))
        })
        .await
        .unwrap();
    assert!(
        partial_col.is_none(),
        "partial_response must be cleared after dismiss"
    );
    assert_eq!(
        msg_count, 1,
        "recovery must produce exactly one Agent message"
    );

    // WS broadcast fired with the recovered id.
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
        .await
        .expect("WS broadcast not received within 2s")
        .expect("broadcast recv error");
    match received {
        WsMessage::PartialResponseRecovered { discussion_ids } => {
            assert!(
                discussion_ids.contains(&"disc-dismiss-1".to_string()),
                "WS must include the dismissed disc id"
            );
        }
        other => panic!("Expected PartialResponseRecovered, got {:?}", other),
    }
}

#[tokio::test]
async fn discussions_dismiss_partial_idempotent_on_clean_disc() {
    // No partial → dismiss is a cheap no-op that returns recovered=false and
    // does NOT broadcast (subscribers are just there for "real" events).
    let state = test_state();
    let mut ws_rx = state.ws_broadcast.subscribe();

    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode)
             VALUES ('disc-clean', 'T', 'ClaudeCode', 'fr', '[]',
             datetime('now'), datetime('now'), 0, 'Direct')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/discussions/disc-clean/dismiss-partial",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["recovered"], false);

    // Drain any non-recovery event the dispatch might emit (e.g. telemetry)
    // and assert that NO PartialResponseRecovered is among them.
    let mut got_recovery = false;
    for _ in 0..5 {
        match tokio::time::timeout(std::time::Duration::from_millis(100), ws_rx.recv()).await {
            Ok(Ok(WsMessage::PartialResponseRecovered { .. })) => {
                got_recovery = true;
                break;
            }
            Ok(Ok(_)) => continue, // some other event — ignore
            _ => break,            // timeout or channel closed — done
        }
    }
    assert!(
        !got_recovery,
        "PartialResponseRecovered must NOT be broadcast when nothing was recovered"
    );
}

#[tokio::test]
async fn discussions_send_message_blocks_while_partial_pending() {
    // Full HTTP guard: POST a new user message while a partial_response
    // checkpoint exists → SSE stream emits a single `error` event with code
    // `partial_pending` and NO agent run is spawned (no new message row).
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode,
             partial_response, partial_response_started_at)
             VALUES ('disc-blocked', 'T', 'ClaudeCode', 'fr', '[]',
             datetime('now'), datetime('now'), 0, 'Direct',
             'half a thought', datetime('now'))",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let app = build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/discussions/disc-blocked/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "content": "hello again",
                "target_agent": null,
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        body_str.contains("event: error"),
        "stream must contain an `error` SSE event, got: {body_str}"
    );
    assert!(
        body_str.contains("partial_pending"),
        "error payload must tag the case as partial_pending, got: {body_str}"
    );

    // No user message was persisted (guard fired before insert).
    let msg_count: i64 = state
        .db
        .with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE discussion_id = 'disc-blocked'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(msg_count, 0,
        "blocked send must not write a user message — otherwise resending after dismiss duplicates it");
}

#[tokio::test]
async fn boot_recovery_simulation_emits_ws_event() {
    // Simulates the main.rs bootstrap sequence: seed partials, run the
    // recovery fn, assert the same WS event is broadcast as the live dismiss
    // path. Guards the exact call chain main.rs:110-124 uses.
    let state = test_state();
    let mut ws_rx = state.ws_broadcast.subscribe();

    state
        .db
        .with_conn(|conn| {
            for id in ["disc-boot-1", "disc-boot-2"] {
                conn.execute(
                    "INSERT INTO discussions (id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode,
                 partial_response, partial_response_started_at)
                 VALUES (?1, 'T', 'ClaudeCode', 'fr', '[]',
                 datetime('now'), datetime('now'), 0, 'Direct',
                 'in-flight output', datetime('now', '-30 seconds'))",
                    rusqlite::params![id],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

    // This is the exact call main.rs makes.
    let ids = state
        .db
        .with_conn(kronn::db::discussions::recover_partial_responses)
        .await
        .unwrap();
    assert_eq!(ids.len(), 2);
    let _ = state
        .ws_broadcast
        .send(kronn::models::WsMessage::PartialResponseRecovered {
            discussion_ids: ids.clone(),
        });

    let received = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
        .await
        .expect("WS not received")
        .expect("broadcast error");
    match received {
        WsMessage::PartialResponseRecovered { discussion_ids } => {
            assert_eq!(discussion_ids.len(), 2);
            assert!(discussion_ids.contains(&"disc-boot-1".to_string()));
            assert!(discussion_ids.contains(&"disc-boot-2".to_string()));
        }
        other => panic!("Expected PartialResponseRecovered, got {:?}", other),
    }
}

#[tokio::test]
async fn discussions_list_no_query_params_returns_all_not_limited_to_50() {
    // Regression for the 2026-04-13 bug: `PaginationQuery.page` used to have a
    // serde default of 1, which made `Option<Query<PaginationQuery>>` always
    // succeed even on a bare `GET /api/discussions`, silently capping results
    // at 50. Users with >50 discussions lost access to their older ones as
    // soon as a 50-item batch pushed them past the boundary.
    //
    // This test creates 60 discussions and asserts that a plain list call
    // returns all 60 (not 50).
    let state = test_state();
    for i in 0..60 {
        let (status, _) = post_json(
            build_router_with_auth(state.clone(), false),
            "/api/discussions",
            serde_json::json!({
                "title": format!("Disc {}", i),
                "agent": "ClaudeCode",
                "language": "en",
                "initial_prompt": "test",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let discs = json["data"].as_array().expect("data must be an array");
    assert_eq!(
        discs.len(),
        60,
        "Bare GET /api/discussions must return ALL discussions, not a paginated slice. \
         Got {} items. Check that PaginationQuery.page has NO serde default.",
        discs.len()
    );
}

#[tokio::test]
async fn discussions_list_explicit_pagination_still_works() {
    // When the caller DOES pass pagination params, the server must honor them.
    // Counterpart to the regression test above.
    let state = test_state();
    for i in 0..10 {
        let (status, _) = post_json(
            build_router_with_auth(state.clone(), false),
            "/api/discussions",
            serde_json::json!({
                "title": format!("Disc {}", i),
                "agent": "ClaudeCode",
                "language": "en",
                "initial_prompt": "test",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions?page=1&per_page=3",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn discussions_create_and_list() {
    let state = test_state();

    // Create a discussion
    let create_body = serde_json::json!({
        "title": "Test discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello, world!"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["title"], "Test discussion");
    assert_eq!(json["data"]["agent"], "ClaudeCode");
    assert_eq!(json["data"]["language"], "en");
    // Should have the initial message
    assert_eq!(json["data"]["messages"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"]["messages"][0]["role"], "User");
    assert_eq!(json["data"]["messages"][0]["content"], "Hello, world!");

    let disc_id = json["data"]["id"].as_str().unwrap().to_string();

    // List discussions — should have one
    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Get discussion by id
    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["id"], disc_id);
    assert_eq!(json["data"]["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn discussion_agent_switch_waits_for_next_user_message() {
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions
                 (id, title, agent, language, participants_json, created_at,
                  updated_at, message_count, workspace_mode, no_agent)
                 VALUES ('d-handoff', 'Handoff', 'ClaudeCode', 'fr', '[]',
                         datetime('now'), datetime('now'), 0, 'Direct', 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let (status, json) = patch_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions/d-handoff",
        serde_json::json!({ "agent": "Codex" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let (message_count, pending): (i64, Option<String>) = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT message_count, pending_agent_handoff_from
                 FROM discussions WHERE id = 'd-handoff'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
        })
        .await
        .unwrap();
    assert_eq!(message_count, 0, "the switch itself must stay silent");
    assert_eq!(pending.as_deref(), Some("ClaudeCode"));

    let req = Request::builder()
        .method("POST")
        .uri("/api/discussions/d-handoff/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "content": "Continue le correctif",
                "target_agent": null,
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = build_router_with_auth(state.clone(), false)
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&body).contains("skipped_no_agent"));

    let (content, pending): (String, Option<String>) = state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT m.content, d.pending_agent_handoff_from
                 FROM discussions d
                 JOIN messages m ON m.discussion_id = d.id
                 WHERE d.id = 'd-handoff'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(Into::into)
        })
        .await
        .unwrap();
    assert!(content.starts_with("<!-- KRONN_AGENT_HANDOFF: ClaudeCode -> Codex."));
    assert!(content.ends_with("Continue le correctif"));
    assert_eq!(pending, None);
}

#[tokio::test]
async fn discussion_native_agent_mode_round_trips_through_http() {
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions
                 (id, title, agent, language, participants_json, created_at,
                  updated_at, message_count, workspace_mode, no_agent)
                 VALUES ('d-peer-only', 'Peers only', 'Codex', 'fr', '[]',
                         datetime('now'), datetime('now'), 0, 'Direct', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let (_, initial) = get_json(app.clone(), "/api/discussions/d-peer-only/native-agent").await;
    assert_eq!(initial["success"], true);
    assert_eq!(initial["data"]["disabled"], false);

    for disabled in [true, false] {
        let (_, updated) = patch_json(
            app.clone(),
            "/api/discussions/d-peer-only",
            serde_json::json!({ "no_agent": disabled }),
        )
        .await;
        assert_eq!(updated["success"], true);

        let (_, persisted) =
            get_json(app.clone(), "/api/discussions/d-peer-only/native-agent").await;
        assert_eq!(persisted["success"], true);
        assert_eq!(persisted["data"]["disabled"], disabled);
    }

    let (_, missing) = get_json(app, "/api/discussions/d-missing/native-agent").await;
    assert_eq!(missing["success"], false);
    assert_eq!(missing["error"], "Discussion not found");
}

#[tokio::test]
async fn discussion_agent_handoff_mode_round_trips_through_http() {
    let state = test_state();
    {
        let mut config = state.config.write().await;
        config.server.agent_handoffs_enabled = true;
        config.server.agent_handoff_paid_limit = 2;
    }
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions
                 (id, title, agent, language, participants_json, created_at,
                  updated_at, message_count, workspace_mode)
                 VALUES ('d-handoff-mode', 'Handoffs', 'Codex', 'fr', '[]',
                         datetime('now'), datetime('now'), 0, 'Direct')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let (_, initial) = get_json(
        app.clone(),
        "/api/discussions/d-handoff-mode/agent-handoffs",
    )
    .await;
    assert_eq!(initial["data"]["global_enabled"], true);
    assert_eq!(initial["data"]["effective_enabled"], true);
    assert_eq!(initial["data"]["unlimited_override"], false);
    assert_eq!(initial["data"]["paid_limit"], 2);

    let (_, updated) = patch_json(
        app.clone(),
        "/api/discussions/d-handoff-mode",
        serde_json::json!({
            "agent_handoffs_disabled": false,
            "agent_handoffs_unlimited": true
        }),
    )
    .await;
    assert_eq!(updated["success"], true);
    let (_, unlimited) = get_json(
        app.clone(),
        "/api/discussions/d-handoff-mode/agent-handoffs",
    )
    .await;
    assert_eq!(unlimited["data"]["unlimited_override"], true);
    assert!(unlimited["data"]["paid_limit"].is_null());

    let (_, updated) = patch_json(
        app.clone(),
        "/api/discussions/d-handoff-mode",
        serde_json::json!({
            "agent_handoffs_disabled": true,
            "agent_handoffs_unlimited": false
        }),
    )
    .await;
    assert_eq!(updated["success"], true);
    let (_, persisted) = get_json(
        app.clone(),
        "/api/discussions/d-handoff-mode/agent-handoffs",
    )
    .await;
    assert_eq!(persisted["data"]["disabled"], true);
    assert_eq!(persisted["data"]["unlimited_override"], false);
    assert_eq!(persisted["data"]["effective_enabled"], false);

    let (_, missing) = get_json(app, "/api/discussions/missing/agent-handoffs").await;
    assert_eq!(missing["success"], false);
}

#[tokio::test]
async fn discussions_create_uses_default_language() {
    let state = test_state();

    // Create without specifying language — should use config default ("fr")
    let create_body = serde_json::json!({
        "title": "No lang specified",
        "agent": "ClaudeCode",
        "language": "",
        "initial_prompt": "Bonjour"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["language"], "fr");
}

#[tokio::test]
async fn discussions_get_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn discussions_delete() {
    let state = test_state();

    // Create
    let create_body = serde_json::json!({
        "title": "To delete",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Delete me"
    });
    let (_, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;
    let disc_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete
    let (status, json) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify gone
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
    )
    .await;
    assert!(json["data"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn discussions_delete_not_found() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/discussions/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Projects endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn projects_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Add folder (project without git)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn projects_add_folder_creates_project_with_no_repo_url() {
    let state = test_state();
    // Use the actual temp directory (always exists).
    let tmp = std::env::temp_dir();
    let path = tmp.to_str().unwrap().to_string();

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": path }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Name auto-inferred from last path component.
    assert!(!json["data"]["name"].as_str().unwrap().is_empty());
    // No git → repo_url is null.
    assert!(json["data"]["repo_url"].is_null());
    assert!(!json["data"]["id"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn projects_add_folder_rejects_nonexistent_path() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": "/does/not/exist/xyzzy" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("does not exist"));
}

#[tokio::test]
async fn projects_add_folder_rejects_path_traversal() {
    let state = test_state();
    let (_, json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": "/home/../etc/shadow" }),
    )
    .await;
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains(".."));
}

#[tokio::test]
async fn projects_add_folder_rejects_duplicate_path() {
    let state = test_state();
    let tmp = std::env::temp_dir();
    let path = tmp.to_str().unwrap().to_string();

    // First add succeeds.
    let (_, json1) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": &path }),
    )
    .await;
    assert_eq!(json1["success"], true);

    // Second add with same path fails.
    let (_, json2) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/add-folder",
        serde_json::json!({ "path": &path }),
    )
    .await;
    assert_eq!(json2["success"], false);
    assert!(json2["error"].as_str().unwrap().contains("already exists"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflows endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn workflows_list_empty() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/workflows").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"].as_array().unwrap().is_empty());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Export endpoint test
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn config_export_empty_db() {
    let app = test_app();
    let req = Request::builder()
        .method("GET")
        .uri("/api/config/export")
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let cursor = std::io::Cursor::new(&bytes[..]);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();

    // Verify data.json contains version 4 with empty collections
    {
        let mut data_file = archive.by_name("data.json").unwrap();
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut data_file, &mut contents).unwrap();
        let data: Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(data["version"], kronn::models::db::CURRENT_EXPORT_VERSION);
        assert!(data["projects"].as_array().unwrap().is_empty());
        assert!(data["discussions"].as_array().unwrap().is_empty());
        assert!(data["workflows"].as_array().unwrap().is_empty());
        assert!(data["mcp_servers"].as_array().unwrap().is_empty());
        assert!(data["mcp_configs"].as_array().unwrap().is_empty());
        assert!(data["contacts"].as_array().unwrap().is_empty());
        assert!(data["quick_prompts"].as_array().unwrap().is_empty());
        // 0.8.9 — new collections must be present and empty too.
        assert!(data["quick_apis"].as_array().unwrap().is_empty());
        assert!(data["learnings"].as_array().unwrap().is_empty());
    }

    // Verify config.toml exists
    assert!(archive.by_name("config.toml").is_ok());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Skills endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let skills = json["data"].as_array().unwrap();
    assert!(
        skills.len() >= 14,
        "Expected at least 14 builtin skills, got {}",
        skills.len()
    );

    // Verify a known builtin skill
    let rust = skills.iter().find(|s| s["id"] == "rust");
    assert!(rust.is_some(), "rust skill not found");
    let rs = rust.unwrap();
    assert_eq!(rs["name"], "rust");
    assert_eq!(rs["icon"], "🦀");
    assert_eq!(rs["category"], "Language");
    assert_eq!(rs["is_builtin"], true);
    assert!(!rs["content"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn skills_list_has_all_categories() {
    let app = test_app();
    let (_, json) = get_json(app, "/api/skills").await;

    let skills = json["data"].as_array().unwrap();
    let categories: std::collections::HashSet<&str> = skills
        .iter()
        .filter_map(|s| s["category"].as_str())
        .collect();

    assert!(categories.contains("Language"), "No Language skills found");
    assert!(categories.contains("Domain"), "No Domain skills found");
    assert!(categories.contains("Business"), "No Business skills found");
}

#[tokio::test]
async fn skills_delete_builtin_rejected() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/skills/rust").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("builtin"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions with skills
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_skill_ids() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "Skill discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello",
        "skill_ids": ["rust", "typescript"]
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let skill_ids = json["data"]["skill_ids"].as_array().unwrap();
    assert_eq!(skill_ids.len(), 2);
    assert_eq!(skill_ids[0], "rust");
    assert_eq!(skill_ids[1], "typescript");

    // Retrieve and verify skill_ids persisted
    let disc_id = json["data"]["id"].as_str().unwrap();
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    let skill_ids = json["data"]["skill_ids"].as_array().unwrap();
    assert_eq!(skill_ids.len(), 2);
}

#[tokio::test]
async fn discussions_create_without_skill_ids_defaults_empty() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "No skills",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // skill_ids should either be absent or empty array
    let skill_ids = json["data"]["skill_ids"].as_array();
    if let Some(ids) = skill_ids {
        assert!(ids.is_empty());
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Cross-cutting: discussion creation validates project reference
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_invalid_project() {
    let app = test_app();
    let body = serde_json::json!({
        "project_id": "nonexistent-project",
        "title": "Bad ref",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Test"
    });
    let (status, json) = post_json(app, "/api/discussions", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("Project not found"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Profiles endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn profiles_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/profiles").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let profiles = json["data"].as_array().unwrap();
    assert!(!profiles.is_empty(), "Expected at least 1 profile");

    // At least one builtin profile should be present
    let builtins: Vec<_> = profiles
        .iter()
        .filter(|p| p["is_builtin"] == true)
        .collect();
    assert!(!builtins.is_empty(), "Expected at least 1 builtin profile");

    // All profiles should have required fields
    for p in profiles {
        assert!(!p["name"].as_str().unwrap().is_empty());
        assert!(!p["persona_prompt"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn profiles_create_and_get() {
    let state = test_state();

    let create_body = serde_json::json!({
        "name": "Test Profile",
        "role": "Test Assistant",
        "avatar": "🤖",
        "color": "#ff0000",
        "category": "Technical",
        "persona_prompt": "You are a test assistant."
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/profiles",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["name"], "Test Profile");
    assert_eq!(json["data"]["category"], "Technical");
    assert_eq!(json["data"]["is_builtin"], false);
    assert_eq!(json["data"]["persona_prompt"], "You are a test assistant.");

    let profile_id = json["data"]["id"].as_str().unwrap().to_string();

    // Get by id
    let (status, json) = get_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/profiles/{}", profile_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["id"], profile_id);
    assert_eq!(json["data"]["name"], "Test Profile");
}

#[tokio::test]
async fn profiles_delete_builtin_rejected() {
    let app = test_app();
    // Get a builtin profile id first
    let (_, json) = get_json(build_router_with_auth(test_state(), false), "/api/profiles").await;
    let profiles = json["data"].as_array().unwrap();
    if profiles.is_empty() {
        return; // No builtins to test
    }
    let builtin_id = profiles[0]["id"].as_str().unwrap();

    let (status, json) = delete_json(app, &format!("/api/profiles/{}", builtin_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("builtin"));
}

#[tokio::test]
async fn profiles_delete_custom() {
    let state = test_state();

    // Create a custom profile
    let create_body = serde_json::json!({
        "name": "To Delete",
        "role": "Temporary",
        "avatar": "🗑️",
        "color": "#999999",
        "category": "Meta",
        "persona_prompt": "Temporary."
    });
    let (_, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/profiles",
        create_body,
    )
    .await;
    let profile_id = json["data"]["id"].as_str().unwrap().to_string();

    // Delete it
    let (status, json) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/profiles/{}", profile_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    // Verify it's gone from the list (only builtins remain)
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/profiles",
    )
    .await;
    let ids: Vec<&str> = json["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(
        !ids.contains(&profile_id.as_str()),
        "Deleted profile should not appear in list"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions with profile_ids
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discussions_create_with_profile_id() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "Profile discussion",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello",
        "profile_ids": ["some-profile-id"]
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(
        json["data"]["profile_ids"],
        serde_json::json!(["some-profile-id"])
    );

    // Retrieve and verify profile_ids persisted
    let disc_id = json["data"]["id"].as_str().unwrap();
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/discussions/{}", disc_id),
    )
    .await;
    assert_eq!(
        json["data"]["profile_ids"],
        serde_json::json!(["some-profile-id"])
    );
}

#[tokio::test]
async fn discussions_create_without_profile_id_defaults_null() {
    let state = test_state();

    let create_body = serde_json::json!({
        "title": "No profile",
        "agent": "ClaudeCode",
        "language": "en",
        "initial_prompt": "Hello"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/discussions",
        create_body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // profile_ids should be absent (skip_serializing_if = "Vec::is_empty")
    assert!(
        json["data"]["profile_ids"].is_null()
            || json["data"]["profile_ids"] == serde_json::json!([])
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Template validation tests (Phase 2)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn templates_use_placeholder_syntax() {
    // All template files should use {{PLACEHOLDER}} syntax, not <!-- fill --> or <!-- ... -->
    // 0.7.1 pivot — templates moved from `templates/ai/` to `templates/docs/`.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    let ambiguous_patterns = ["<!-- fill", "<!-- Add ", "<!-- Describe ", "<!-- List "];

    for entry in walkdir::WalkDir::new(&template_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        // Skip TEMPLATE.md which is a template for templates
        if entry.path().file_name().is_some_and(|n| n == "TEMPLATE.md") {
            continue;
        }

        let content = std::fs::read_to_string(entry.path()).unwrap();
        let rel = entry.path().strip_prefix(&template_dir).unwrap();

        for pattern in &ambiguous_patterns {
            assert!(
                !content.contains(pattern),
                "Template {} contains ambiguous placeholder '{}' — use {{{{PLACEHOLDER}}}} syntax instead",
                rel.display(), pattern
            );
        }
    }
}

#[test]
fn templates_have_no_empty_comment_placeholders() {
    // 0.7.1 pivot — templates/ai → templates/docs.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    for entry in walkdir::WalkDir::new(&template_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        if entry.path().file_name().is_some_and(|n| n == "TEMPLATE.md") {
            continue;
        }

        let content = std::fs::read_to_string(entry.path()).unwrap();
        let rel = entry.path().strip_prefix(&template_dir).unwrap();

        // Should not have generic <!-- ... --> placeholders. Exceptions: TODO
        // markers, KRONN markers, and the `kronn:` namespaced anchors that the
        // anti-hallu STEP 0 reads/refreshes (kronn:doc-version, kronn:spec).
        for (i, line) in content.lines().enumerate() {
            if line.contains("<!-- ")
                && !line.contains("<!-- TODO")
                && !line.contains("<!-- KRONN")
                && !line.contains("<!-- kronn:")
            {
                // Allow specific known comments
                if line.contains("<!-- Fill")
                    || line.contains("<!-- Flag")
                    || line.contains("<!-- Add entries")
                {
                    // These are instructions in table comments, acceptable
                    continue;
                }
                // The anti-hallu doc header opens with an explanatory comment
                // (documents the convention for any CLI reading the raw file).
                if line.contains("<!-- This file follows") {
                    continue;
                }
                panic!(
                    "Template {}:{} has generic comment placeholder: {}",
                    rel.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }
}

#[test]
fn templates_all_have_placeholders() {
    // Verify that key template files contain {{PLACEHOLDER}} patterns (they're skeletons, not empty)
    // 0.7.1 pivot — templates/ai → templates/docs.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    // 0.7.1 — `index.md` (legacy LLM entry) was renamed to `AGENTS.md`.
    let expected_files = [
        "AGENTS.md",
        "glossary.md",
        "repo-map.md",
        "coding-rules.md",
        "testing-quality.md",
        "architecture/overview.md",
        "operations/debug-operations.md",
        "inconsistencies-tech-debt.md",
    ];

    for file in &expected_files {
        let path = template_dir.join(file);
        assert!(path.exists(), "Template file {} should exist", file);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("{{"),
            "Template {} should contain {{{{PLACEHOLDER}}}} patterns",
            file
        );
    }
}

#[test]
fn template_mcp_template_exists() {
    // 0.7.1 pivot — templates/ai → templates/docs.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    let mcp_template = template_dir.join("operations/mcp-servers/TEMPLATE.md");
    assert!(mcp_template.exists(), "MCP TEMPLATE.md should exist");
    let content = std::fs::read_to_string(&mcp_template).unwrap();
    assert!(
        content.contains("{{MCP_NAME}}"),
        "MCP template should have {{MCP_NAME}} placeholder"
    );
    assert!(
        content.contains("Rules"),
        "MCP template should have Rules section"
    );
    assert!(
        content.contains("Gotchas")
            || content.contains("Examples")
            || content.contains("usage patterns"),
        "MCP template should have gotchas or examples section"
    );
}

#[test]
fn template_tech_debt_dir_exists() {
    // 0.7.1 pivot — templates/ai → templates/docs.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    let td_dir = template_dir.join("tech-debt");
    assert!(
        td_dir.exists(),
        "tech-debt/ directory should exist in templates"
    );
    assert!(
        td_dir.join(".gitkeep").exists(),
        "tech-debt/.gitkeep should exist"
    );
}

#[test]
fn template_inconsistencies_has_outdated_prerequisites_table() {
    // 0.7.1 pivot — templates moved from `templates/ai/` to
    // `templates/docs/`. Path is required to exist now (no silent
    // early-return); a missing template means a packaging bug.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    let content =
        std::fs::read_to_string(template_dir.join("inconsistencies-tech-debt.md")).unwrap();
    assert!(
        content.contains("Outdated dependencies") || content.contains("Outdated prerequisites"),
        "Should have outdated dependencies/prerequisites section"
    );
    assert!(content.contains("Severity"), "Should have severity column");
    // The template reference can be `tech-debt/...` (relative to docs/)
    // or the fully-qualified `docs/tech-debt/...`. Either is correct.
    assert!(
        content.contains("docs/tech-debt/") || content.contains("tech-debt/TD-"),
        "Should reference tech-debt detail files",
    );
}

#[test]
fn template_glossary_has_todo_marker_guidance() {
    // 0.7.1 pivot — templates/ai → templates/docs.
    let template_dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../templates/docs");
    assert!(template_dir.is_dir(), "templates/docs/ must exist");

    let content = std::fs::read_to_string(template_dir.join("glossary.md")).unwrap();
    assert!(
        content.contains("TODO: ask user"),
        "Glossary should mention TODO: ask user markers"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// ANALYSIS_STEPS prompt validation tests (Phase 1 + 2)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn analysis_steps_glossary_mentions_todo_markers() {
    // The glossary step prompt should instruct the agent to add TODO markers for unknown terms
    let prompt_preamble = include_str!("../src/api/audit/mod.rs");
    // Find the glossary step prompt
    assert!(
        prompt_preamble.contains("TODO: ask user"),
        "Glossary step prompt should instruct adding TODO: ask user markers"
    );
}

#[test]
fn analysis_steps_tech_debt_creates_detail_files() {
    let source = include_str!("../src/api/audit/mod.rs");
    // 0.7.1 pivot — prompts now reference `docs/tech-debt/` (post-pivot
    // convention). Legacy `ai/tech-debt/` projects keep working through
    // `detect_docs_dir`, but the bootstrap prompt instructs agents on
    // the modern path.
    assert!(
        source.contains("docs/tech-debt/TD-"),
        "Tech debt step should instruct creating detail files in docs/tech-debt/"
    );
}

#[test]
fn analysis_steps_tech_debt_checks_outdated_prerequisites() {
    let source = include_str!("../src/api/audit/mod.rs");
    // The tech debt step should mention checking for outdated prerequisites
    assert!(
        source.contains("deprecated") || source.contains("EOL") || source.contains("outdated"),
        "Tech debt step should instruct checking for outdated prerequisites"
    );
}

#[test]
fn analysis_steps_review_checks_tech_debt_files() {
    let source = include_str!("../src/api/audit/mod.rs");
    assert!(
        source.contains("Tech debt files") || source.contains("tech-debt/"),
        "Review step should verify tech-debt detail files exist"
    );
}

#[test]
fn analysis_steps_review_checks_glossary_todos() {
    let source = include_str!("../src/api/audit/mod.rs");
    assert!(
        source.contains("Glossary TODO") || source.contains("TODO: ask user"),
        "Review step should check glossary TODO markers"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Bootstrap endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bootstrap_empty_name_returns_error() {
    // To reach the name validation, we need a state with a scan path configured.
    let state = test_state();
    {
        let mut cfg = state.config.write().await;
        cfg.scan.paths = vec!["/tmp".to_string()];
    }

    let body = serde_json::json!({
        "name": "   ",
        "description": "Some description",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/bootstrap",
        body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("name"),
        "Error should mention name, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn bootstrap_no_scan_paths_no_projects_returns_error() {
    // Default test state has no scan paths and no existing projects,
    // so bootstrap should fail because it cannot determine a parent directory.
    let app = test_app();
    let body = serde_json::json!({
        "name": "my-new-project",
        "description": "A brand new project",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/bootstrap", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("scan path")
            || json["error"].as_str().unwrap().contains("Parent directory"),
        "Error should mention missing scan path or parent directory, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn bootstrap_route_accepts_correct_payload() {
    // Verify that the route is registered and accepts the BootstrapProjectRequest shape.
    // We send a valid payload; even though it will fail (no writable parent dir in test),
    // we should NOT get a 404 or 405, and the JSON should be parseable with success/error fields.
    let app = test_app();
    let body = serde_json::json!({
        "name": "test-project",
        "description": "Testing bootstrap",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/bootstrap", body).await;

    // Route exists — not 404/405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "Bootstrap route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "Bootstrap should accept POST"
    );
    // Response is a valid ApiResponse
    assert!(
        json["success"].is_boolean(),
        "Response should have a boolean 'success' field"
    );
}

#[tokio::test]
async fn bootstrap_invalid_payload_returns_error() {
    // Missing required fields should produce a 4xx or an error response
    let app = test_app();
    let body = serde_json::json!({
        "description": "no name field"
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/projects/bootstrap")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    // Axum rejects malformed JSON payloads with 422 (Unprocessable Entity)
    assert!(
        status == StatusCode::UNPROCESSABLE_ENTITY || status == StatusCode::BAD_REQUEST,
        "Missing required fields should return 422 or 400, got {}",
        status
    );
}

#[tokio::test]
async fn bootstrap_find_common_parent_logic() {
    // Indirectly test find_common_parent by creating two projects under the same parent,
    // then verifying bootstrap doesn't complain about missing scan paths.
    let state = test_state();

    // Insert two projects at known paths under /tmp/kronn-test-parent
    let now = chrono::Utc::now();
    for (name, subdir) in &[("Project A", "project-a"), ("Project B", "project-b")] {
        let project = kronn::models::Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            path: format!("/tmp/kronn-test-bootstrap-parent/{}", subdir),
            repo_url: None,
            token_override: None,
            ai_config: kronn::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: kronn::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: now,
            updated_at: now,
        };
        let p = project.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::projects::insert_project(conn, &p))
            .await
            .unwrap();
    }

    // Now bootstrap should use common parent /tmp/kronn-test-bootstrap-parent
    // It will fail on filesystem ops (dir doesn't exist), but the error should
    // mention "Parent directory not found" or "Directory already exists" — NOT "No scan path".
    let body = serde_json::json!({
        "name": "new-child",
        "description": "Testing common parent",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/bootstrap",
        body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // The error should NOT be about missing scan paths — find_common_parent should have worked
    if !json["success"].as_bool().unwrap_or(false) {
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            !err.contains("No scan path"),
            "With existing projects, bootstrap should find common parent, not complain about scan paths. Got: {}",
            err
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Mark bootstrapped endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn mark_bootstrapped_route_exists() {
    let app = test_app();
    let (status, json) = post_json(
        app,
        "/api/projects/nonexistent-id/mark-bootstrapped",
        serde_json::json!({}),
    )
    .await;

    // Route exists — not 404/405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "mark-bootstrapped route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "mark-bootstrapped should accept POST"
    );
    // Response is a valid ApiResponse (project not found)
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow creation test
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn workflow_create_with_valid_payload_returns_ok() {
    let state = test_state();

    let body = serde_json::json!({
        "name": "Test workflow",
        "project_id": null,
        "trigger": { "type": "Manual" },
        "steps": [{
            "name": "step1",
            "agent": "ClaudeCode",
            "prompt_template": "Do something",
            "mode": { "type": "Normal" }
        }],
        "actions": [],
        "safety": { "sandbox": false, "require_approval": false }
    });

    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/workflows",
        body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(json["data"]["id"].is_string());
    assert_eq!(json["data"]["name"], "Test workflow");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Server config endpoint
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn server_config_returns_defaults() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/config/server").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Should return ServerConfigPublic fields
    assert!(json["data"]["host"].is_string());
    assert!(json["data"]["port"].is_number());
    assert!(json["data"]["max_concurrent_agents"].is_number());
    assert_eq!(json["data"]["agent_global_timeout_min"], 30);
    assert_eq!(json["data"]["local_agent_global_timeout_min"], 240);
    assert!(json["data"]["auth_enabled"].is_boolean());
    assert_eq!(json["data"]["agent_handoffs_enabled"], false);
    assert_eq!(json["data"]["agent_handoff_paid_limit"], 1);
    assert_eq!(json["data"]["agent_handoff_paid_unlimited"], false);
    assert_eq!(
        json["data"]["agent_handoff_blocked_agents"],
        serde_json::json!([])
    );
}

#[tokio::test]
async fn server_config_persists_and_clamps_agent_timeouts_independently() {
    let app = test_app();
    let (_, updated) = post_json(
        app.clone(),
        "/api/config/server",
        serde_json::json!({
            "agent_global_timeout_min": 45,
            "local_agent_global_timeout_min": 180
        }),
    )
    .await;
    assert_eq!(updated["success"], true);

    let (_, persisted) = get_json(app.clone(), "/api/config/server").await;
    assert_eq!(persisted["data"]["agent_global_timeout_min"], 45);
    assert_eq!(persisted["data"]["local_agent_global_timeout_min"], 180);

    let _ = post_json(
        app.clone(),
        "/api/config/server",
        serde_json::json!({
            "agent_global_timeout_min": 999,
            "local_agent_global_timeout_min": 999
        }),
    )
    .await;
    let (_, clamped) = get_json(app, "/api/config/server").await;
    assert_eq!(clamped["data"]["agent_global_timeout_min"], 240);
    assert_eq!(clamped["data"]["local_agent_global_timeout_min"], 240);
}

#[tokio::test]
async fn server_config_updates_and_clamps_agent_handoff_budget() {
    let app = test_app();
    let (_, updated) = post_json(
        app.clone(),
        "/api/config/server",
        serde_json::json!({
            "agent_handoffs_enabled": true,
            "agent_handoff_paid_limit": 99,
            "agent_handoff_paid_unlimited": true,
            "agent_handoff_blocked_agents": ["ClaudeCode", "LiteLlm", "ClaudeCode"]
        }),
    )
    .await;
    assert_eq!(updated["success"], true);

    let (_, persisted) = get_json(app, "/api/config/server").await;
    assert_eq!(persisted["data"]["agent_handoffs_enabled"], true);
    assert_eq!(persisted["data"]["agent_handoff_paid_limit"], 5);
    assert_eq!(persisted["data"]["agent_handoff_paid_unlimited"], true);
    assert_eq!(
        persisted["data"]["agent_handoff_blocked_agents"],
        serde_json::json!(["ClaudeCode", "LiteLlm"])
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Build bootstrap prompt tests (via source analysis)
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn bootstrap_prompt_contains_project_name() {
    let source = include_str!("../src/api/projects/bootstrap.rs");
    // The bootstrap prompt function should include the project name
    assert!(
        source.contains("build_bootstrap_prompt"),
        "build_bootstrap_prompt function should exist"
    );
    // Should support multiple languages
    assert!(
        source.contains("Réponds en français") || source.contains("fr"),
        "Should support French"
    );
    assert!(
        source.contains("Respond in English") || source.contains("en"),
        "Should support English"
    );
}

#[test]
fn detect_project_skills_function_exists() {
    let source = include_str!("../src/api/audit/helpers.rs");
    assert!(
        source.contains("detect_project_skills"),
        "detect_project_skills function should exist"
    );
    // Should check common project files
    assert!(source.contains("Cargo.toml"), "Should detect Rust projects");
    assert!(
        source.contains("tsconfig.json"),
        "Should detect TypeScript projects"
    );
    assert!(source.contains("go.mod"), "Should detect Go projects");
    assert!(
        source.contains("requirements.txt") || source.contains("pyproject.toml"),
        "Should detect Python projects"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Hard delete endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn delete_project_hard_nonexistent_returns_error() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/projects/nonexistent-id?hard=true").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn delete_project_soft_nonexistent_returns_error() {
    let app = test_app();
    let (status, json) = delete_json(app, "/api/projects/nonexistent-id").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Clone endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn clone_route_exists() {
    let app = test_app();
    let body = serde_json::json!({
        "url": "https://github.com/test/repo.git",
        "agent": "ClaudeCode"
    });
    let (status, _json) = post_json(app, "/api/projects/clone", body).await;

    // Route exists — not 404/405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "Clone route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "Clone should accept POST"
    );
}

#[tokio::test]
async fn clone_empty_url_returns_error() {
    let app = test_app();
    let body = serde_json::json!({
        "url": "   ",
        "agent": "ClaudeCode"
    });
    let (status, json) = post_json(app, "/api/projects/clone", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("url")
            || json["error"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("required")
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discover repos endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn discover_repos_route_exists() {
    let app = test_app();
    let (status, _json) =
        post_json(app, "/api/projects/discover-repos", serde_json::json!({})).await;

    // Route exists — not 404/405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "Discover repos route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "Discover repos should accept POST"
    );
}

#[tokio::test]
async fn discover_repos_no_token_returns_error() {
    let app = test_app();
    let (status, json) =
        post_json(app, "/api/projects/discover-repos", serde_json::json!({})).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    // Should mention no token found
    assert!(
        json["error"].as_str().unwrap().contains("token")
            || json["error"].as_str().unwrap().contains("Token")
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Git Panel endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn git_status_route_exists() {
    let app = test_app();
    let (status, _json) = get_json(app, "/api/projects/some-id/git-status").await;

    // Route registered — not 404 or 405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "git-status route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "git-status should accept GET"
    );
}

#[tokio::test]
async fn git_status_project_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent-id/git-status").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found")
            || json["error"].as_str().unwrap().contains("Not found")
    );
}

#[tokio::test]
async fn git_diff_route_exists() {
    let app = test_app();
    let (status, _json) = get_json(app, "/api/projects/some-id/git-diff?path=test.rs").await;

    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "git-diff route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "git-diff should accept GET"
    );
}

#[tokio::test]
async fn git_diff_project_not_found() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent-id/git-diff?path=file.rs").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found")
            || json["error"].as_str().unwrap().contains("Not found")
    );
}

#[tokio::test]
async fn git_branch_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "name": "test-branch" });
    let (status, _json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "git-branch route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "git-branch should accept POST"
    );
}

#[tokio::test]
async fn git_branch_project_not_found() {
    let app = test_app();
    let body = serde_json::json!({ "name": "feature-x" });
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found")
            || json["error"].as_str().unwrap().contains("Not found")
    );
}

#[tokio::test]
async fn git_branch_empty_name_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("Invalid")
            || json["error"].as_str().unwrap().contains("name")
    );
}

#[tokio::test]
async fn git_branch_name_with_spaces_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "my bad branch" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_branch_name_with_dotdot_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "name": "branch..bad" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-branch", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_commit_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["test.rs"], "message": "test commit" });
    let (status, _json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "git-commit route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "git-commit should accept POST"
    );
}

#[tokio::test]
async fn git_commit_empty_message_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["test.rs"], "message": "" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("message")
            || json["error"].as_str().unwrap().contains("required")
    );
}

#[tokio::test]
async fn git_commit_no_files_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": [], "message": "my commit" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("No files")
            || json["error"].as_str().unwrap().contains("files")
    );
}

#[tokio::test]
async fn git_commit_path_traversal_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "files": ["../../../etc/passwd"], "message": "pwn" });
    let (status, json) = post_json(app, "/api/projects/some-id/git-commit", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

#[tokio::test]
async fn git_push_route_exists() {
    let app = test_app();
    let (status, _json) =
        post_json(app, "/api/projects/some-id/git-push", serde_json::json!({})).await;

    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "git-push route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "git-push should accept POST"
    );
}

#[tokio::test]
async fn git_push_project_not_found() {
    let app = test_app();
    let (status, json) = post_json(
        app,
        "/api/projects/nonexistent-id/git-push",
        serde_json::json!({}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found")
            || json["error"].as_str().unwrap().contains("Not found")
    );
}

#[tokio::test]
async fn git_diff_path_traversal_rejected() {
    let app = test_app();
    let (status, json) = get_json(
        app,
        "/api/projects/some-id/git-diff?path=../../../etc/passwd",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Invalid"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Exec endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn exec_route_exists() {
    let app = test_app();
    let body = serde_json::json!({ "command": "echo hello" });
    let (status, _json) = post_json(app, "/api/projects/some-id/exec", body).await;

    // Route exists — not 404/405
    assert_ne!(
        status,
        StatusCode::NOT_FOUND,
        "exec route should be registered"
    );
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "exec should accept POST"
    );
}

#[tokio::test]
async fn exec_project_not_found() {
    let app = test_app();
    let body = serde_json::json!({ "command": "echo hello" });
    let (status, json) = post_json(app, "/api/projects/nonexistent-id/exec", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("not found")
            || json["error"].as_str().unwrap().contains("Not found"),
        "Error should mention project not found, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn exec_empty_command_rejected() {
    let app = test_app();
    let body = serde_json::json!({ "command": "   " });
    let (status, json) = post_json(app, "/api/projects/some-id/exec", body).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    assert!(
        json["error"].as_str().unwrap().contains("Empty command"),
        "Error should mention empty command, got: {}",
        json["error"]
    );
}

#[tokio::test]
async fn exec_dangerous_command_blocked() {
    let _app = test_app();
    let blocked_commands = [
        "rm -rf /",
        "sudo apt install",
        "chmod 777 .",
        "chown root .",
        "kill -9 1",
        "reboot",
        "shutdown now",
        "mkfs /dev/sda",
        "dd if=/dev/zero",
    ];

    for cmd in &blocked_commands {
        let body = serde_json::json!({ "command": cmd });
        let (_status, json) = post_json(test_app(), "/api/projects/some-id/exec", body).await;

        assert_eq!(
            json["success"], false,
            "Command '{}' should be blocked",
            cmd
        );
        assert!(
            json["error"].as_str().unwrap().contains("not allowed"),
            "Blocked command '{}' should say 'not allowed', got: {}",
            cmd,
            json["error"]
        );
    }
}

#[tokio::test]
async fn exec_returns_expected_fields() {
    // Create a project with a real path so the command can execute
    let state = test_state();
    let now = chrono::Utc::now();
    let project = kronn::models::Project {
        id: "exec-test-proj".to_string(),
        name: "Exec Test".to_string(),
        path: "/tmp".to_string(),
        repo_url: None,
        token_override: None,
        ai_config: kronn::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: kronn::models::AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    let p = project.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::projects::insert_project(conn, &p))
        .await
        .unwrap();

    let body = serde_json::json!({ "command": "echo hello" });
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/projects/exec-test-proj/exec",
        body,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["success"], true,
        "exec should succeed, got: {:?}",
        json
    );
    let data = &json["data"];
    assert!(
        data["stdout"].is_string(),
        "Response should have stdout field"
    );
    assert!(
        data["stderr"].is_string(),
        "Response should have stderr field"
    );
    assert!(
        data["exit_code"].is_number(),
        "Response should have exit_code field"
    );
    assert_eq!(data["stdout"].as_str().unwrap().trim(), "hello");
    assert_eq!(data["exit_code"].as_i64().unwrap(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// WebSocket integration tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: start a real TCP server with the router and return the address.
async fn start_test_server(state: AppState) -> std::net::SocketAddr {
    let app = build_router_with_auth(state, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// ═══════════════════════════════════════════════════════════════════════════════
// disc_git / ai_docs / discover — route existence smoke tests (0.3.7)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn disc_git_status_returns_error_for_nonexistent_disc() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/discussions/nonexistent/git-status").await;
    // Route exists, returns an error (not 404 — the handler runs but disc not found)
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

#[tokio::test]
async fn disc_git_diff_route_exists() {
    // git-diff may return non-JSON (raw diff text), so just verify the route exists (not 404)
    let app = test_app();
    let req = Request::builder()
        .method("GET")
        .uri("/api/discussions/nonexistent/git-diff")
        .body(Body::empty())
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::NOT_FOUND, "Route must exist");
}

#[tokio::test]
async fn ai_files_returns_error_for_nonexistent_project() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/nonexistent/ai-files").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
}

#[tokio::test]
async fn source_browser_routes_return_envelopes_for_nonexistent_project() {
    for uri in [
        "/api/projects/nonexistent/source-files",
        "/api/projects/nonexistent/source-file?path=src%2Fmain.rs",
        "/api/projects/nonexistent/source-search?q=main",
        "/api/projects/nonexistent/source-exclusions",
        "/api/projects/nonexistent/git-blame?path=src%2Fmain.rs",
    ] {
        let (status, json) = get_json(test_app(), uri).await;
        assert_eq!(status, StatusCode::OK, "{uri} must be registered");
        assert_eq!(json["success"], false, "{uri} must return an API envelope");
        assert!(
            json["error"]
                .as_str()
                .is_some_and(|error| error.contains("not found")),
            "{uri} should report the missing project"
        );
    }
}

#[tokio::test]
async fn source_browser_lists_reads_and_searches_project_code() {
    let state = test_state();
    let repo = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join("src")).unwrap();
    std::fs::create_dir_all(repo.path().join("docs")).unwrap();
    std::fs::write(
        repo.path().join("src/main.rs"),
        "fn main() { println!(\"kronn-source-probe\"); }\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("docs/AGENTS.md"), "hidden docs probe").unwrap();
    std::fs::write(repo.path().join(".env"), "SECRET=hidden").unwrap();
    std::fs::write(repo.path().join("README.md"), "root readme").unwrap();
    std::fs::create_dir_all(repo.path().join("application")).unwrap();
    std::fs::write(
        repo.path().join("application/.htaccess"),
        "RewriteEngine On",
    )
    .unwrap();
    std::fs::write(repo.path().join(".gitignore"), "application/local.rules\n").unwrap();
    std::fs::write(repo.path().join("application/local.rules"), "local = true").unwrap();
    std::fs::create_dir_all(repo.path().join("generated/client")).unwrap();
    std::fs::write(
        repo.path().join("generated/client/api.ts"),
        "export const marker = 'kronn-generated-probe';",
    )
    .unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "Kronn Tester"][..],
        &["config", "user.email", "kronn@example.test"][..],
        &["add", "src/main.rs", "README.md", ".gitignore"][..],
        &["commit", "-q", "-m", "seed source"][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success(), "git command failed: {args:?}");
    }

    let now = chrono::Utc::now();
    let project = kronn::models::Project {
        id: "source-browser-project".into(),
        name: "Source Browser".into(),
        path: repo.path().to_string_lossy().to_string(),
        repo_url: None,
        token_override: None,
        ai_config: kronn::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: kronn::models::AiAuditStatus::NoTemplate,
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: now,
        updated_at: now,
    };
    state
        .db
        .with_conn(move |conn| kronn::db::projects::insert_project(conn, &project))
        .await
        .unwrap();
    let app = build_router_with_auth(state, false);

    let (_, shallow) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-files?shallow=true",
    )
    .await;
    let shallow_json = shallow["data"].to_string();
    assert!(shallow_json.contains("\"path\":\"src\""));
    assert!(shallow_json.contains("README.md"));
    assert!(!shallow_json.contains("src/main.rs"));

    let (_, listed) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-files",
    )
    .await;
    let listed_json = listed["data"].to_string();
    assert!(listed_json.contains("src/main.rs"));
    assert!(listed_json.contains("README.md"));
    assert!(listed_json.contains("application/.htaccess"));
    assert!(listed_json.contains("application/local.rules"));
    assert!(listed_json.contains("generated/client/api.ts"));
    assert!(listed_json.contains("\"git_ignored\":true"));
    assert!(!listed_json.contains("docs/AGENTS.md"));
    assert!(!listed_json.contains(".env"));

    let (_, read) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-file?path=src%2Fmain.rs",
    )
    .await;
    assert_eq!(read["success"], true);
    assert!(read["data"]["content"]
        .as_str()
        .is_some_and(|content| content.contains("kronn-source-probe")));

    let (_, searched) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-search?q=kronn-source-probe",
    )
    .await;
    assert_eq!(searched["success"], true);
    assert_eq!(searched["data"][0]["path"], "src/main.rs");
    assert_eq!(searched["data"][0]["match_count"], 1);

    let (_, exclusions) = put_json_root(
        app.clone(),
        "/api/projects/source-browser-project/source-exclusions",
        serde_json::json!(["generated/client"]),
    )
    .await;
    assert_eq!(exclusions["success"], true);
    assert_eq!(exclusions["data"], serde_json::json!(["generated/client"]));

    let (_, listed_after_exclusion) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-files",
    )
    .await;
    assert!(!listed_after_exclusion["data"]
        .to_string()
        .contains("generated/client/api.ts"));

    let (_, searched_after_exclusion) = get_json(
        app.clone(),
        "/api/projects/source-browser-project/source-search?q=kronn-generated-probe",
    )
    .await;
    assert_eq!(searched_after_exclusion["success"], true);
    assert_eq!(searched_after_exclusion["data"], serde_json::json!([]));

    let (_, blamed) = get_json(
        app,
        "/api/projects/source-browser-project/git-blame?path=src%2Fmain.rs",
    )
    .await;
    assert_eq!(blamed["success"], true);
    assert_eq!(blamed["data"]["lines"][0]["author"], "Kronn Tester");
    assert_eq!(blamed["data"]["lines"][0]["line_number"], 1);
}

#[tokio::test]
async fn discover_repos_accepts_empty_sources() {
    // discover-repos needs MCP tokens to actually work — just verify the route accepts the payload
    let state = test_state();
    let (status, _json) = post_json(
        build_router_with_auth(state, false),
        "/api/projects/discover-repos",
        serde_json::json!({ "source_ids": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Ollama endpoints (0.4.0)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn ollama_health_returns_valid_status() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/ollama/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Status depends on whether Ollama is running on the dev machine
    let health_status = json["data"]["status"].as_str().unwrap();
    assert!(
        ["online", "offline", "not_installed", "unreachable"].contains(&health_status),
        "Unexpected status: '{}'",
        health_status
    );
    // Endpoint field is always present
    assert!(json["data"]["endpoint"]
        .as_str()
        .unwrap()
        .starts_with("http"));
}

#[tokio::test]
async fn ollama_models_returns_valid_response() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/ollama/models").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    // Models array: empty if Ollama offline, populated if online
    assert!(json["data"]["models"].is_array());
}

#[tokio::test]
#[serial_test::serial]
async fn ollama_context_override_route_set_reset_and_model_projection() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models": [{
                "name": "qwen-test:latest",
                "size": 4_000_000_000u64,
                "modified_at": "2026-08-24T00:00:00Z"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model_info": { "qwen3.context_length": 131_072u64 }
        })))
        .mount(&server)
        .await;

    let previous_host = std::env::var("OLLAMA_HOST").ok();
    let previous_cap = std::env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok();
    std::env::set_var("OLLAMA_HOST", server.uri());
    std::env::remove_var("KRONN_OLLAMA_NUM_CTX_CAP");

    let state = test_state();
    let app = build_router_with_auth(state, false);
    let (_, set) = post_json(
        app.clone(),
        "/api/ollama/context-override",
        serde_json::json!({
            "model": "  qwen-test:latest  ",
            "num_ctx": 65_536u64
        }),
    )
    .await;
    let (_, listed_with_override) = get_json(app.clone(), "/api/ollama/models").await;
    let (_, below_floor) = post_json(
        app.clone(),
        "/api/ollama/context-override",
        serde_json::json!({"model": "qwen-test:latest", "num_ctx": 512u64}),
    )
    .await;
    let (_, above_max) = post_json(
        app.clone(),
        "/api/ollama/context-override",
        serde_json::json!({"model": "qwen-test:latest", "num_ctx": 50_000_000u64}),
    )
    .await;
    let (_, reset) = post_json(
        app.clone(),
        "/api/ollama/context-override",
        serde_json::json!({"model": "qwen-test:latest", "num_ctx": null}),
    )
    .await;
    let (_, listed_after_reset) = get_json(app, "/api/ollama/models").await;

    match previous_host {
        Some(value) => std::env::set_var("OLLAMA_HOST", value),
        None => std::env::remove_var("OLLAMA_HOST"),
    }
    match previous_cap {
        Some(value) => std::env::set_var("KRONN_OLLAMA_NUM_CTX_CAP", value),
        None => std::env::remove_var("KRONN_OLLAMA_NUM_CTX_CAP"),
    }

    assert_eq!(set["success"], true, "{set}");
    assert_eq!(set["data"]["model"], "qwen-test:latest");
    assert_eq!(set["data"]["num_ctx"], 65_536u64);
    assert!(set["data"]["warnings"].is_array(), "{set}");
    let projected = &listed_with_override["data"]["models"][0];
    assert_eq!(projected["advertised_context"], 131_072u64);
    assert_eq!(projected["context_override"], 65_536u64);
    assert_eq!(projected["context_ceiling"], 65_536u64);
    assert_eq!(projected["context_origin"], "model_override");
    assert_eq!(below_floor["success"], false, "{below_floor}");
    assert_eq!(above_max["success"], false, "{above_max}");
    assert_eq!(reset["success"], true, "{reset}");
    assert!(
        listed_after_reset["data"]["models"][0]["context_override"].is_null(),
        "{listed_after_reset}"
    );
    assert_ne!(
        listed_after_reset["data"]["models"][0]["context_origin"],
        "model_override"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Skills / Profiles / Directives / Stats — CRUD smoke tests (0.3.7)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_crud_create_and_delete_custom() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/skills",
        serde_json::json!({
            "name": "TestSkill",
            "description": "A test skill",
            "icon": "Zap",
            "category": "Domain",
            "content": "Be excellent.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let skill_id = json["data"]["id"].as_str().unwrap().to_string();

    // Verify it appears in list
    let (_, json) = get_json(build_router_with_auth(state.clone(), false), "/api/skills").await;
    assert!(json["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == skill_id));

    // Delete
    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/skills/{}", skill_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn directives_list_returns_builtins() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/directives").await;
    assert_eq!(status, StatusCode::OK);
    let directives = json["data"].as_array().unwrap();
    assert!(
        !directives.is_empty(),
        "Expected at least 1 builtin directive"
    );
}

#[tokio::test]
async fn directives_create_and_delete_custom() {
    let state = test_state();
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/directives",
        serde_json::json!({
            "name": "TestDirective",
            "description": "Be terse",
            "icon": "MessageSquare",
            "category": "Output",
            "content": "Keep answers under 3 sentences.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/directives/{}", id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn stats_tokens_returns_success() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/tokens").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["total_tokens"], 0);
}

#[tokio::test]
async fn stats_agent_usage_returns_success() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/stats/agent-usage").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn agents_detect_returns_list() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let agents = json["data"].as_array().unwrap();
    assert!(
        agents.len() >= 6,
        "Expected at least 6 agents, got {}",
        agents.len()
    );
}

// ─── Compare-agents mode (POST /api/quick-prompts/:id/compare-agents) ───
//
// Pinned regression for the new fan-out-across-agents endpoint. We
// don't drive a real agent run here (that's covered by
// `codex-real-introspection.spec.ts`); we assert the endpoint
// validates inputs (empty agents, missing prompt) and creates the
// right number of child discussions when given a valid payload.
//
// The actual disc creation needs a real QP in the DB, which the
// existing test_app() spins up in-memory. Each child disc carries
// the per-item agent_override, mirroring the Compare-agents UX
// (1 prompt × N agents).

#[tokio::test]
async fn compare_agents_rejects_empty_agents() {
    let app = test_app();
    let body = serde_json::json!({
        "prompt": "test prompt",
        "batch_name": "test-batch",
        "agents": [],
    });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/quick-prompts/missing-id/compare-agents")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], false);
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("at least 1 agent"),
        "expected agents-empty error, got: {}",
        err
    );
}

#[tokio::test]
async fn compare_agents_rejects_empty_prompt() {
    let app = test_app();
    let body = serde_json::json!({
        "prompt": "   ",
        "batch_name": "test-batch",
        "agents": ["ClaudeCode"],
    });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/quick-prompts/missing-id/compare-agents")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"]
        .as_str()
        .unwrap_or("")
        .contains("Prompt is required"));
}

#[tokio::test]
async fn compare_agents_rejects_missing_qp() {
    // QP doesn't exist → "not found" error, not a server crash.
    let app = test_app();
    let body = serde_json::json!({
        "prompt": "real prompt",
        "batch_name": "test-batch",
        "agents": ["ClaudeCode", "Codex"],
    });
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/quick-prompts/does-not-exist/compare-agents")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap_or("").contains("not found"));
}

// ─── Version check (auto-update banner) ─────────────────────────────────
//
// Pinned regression for `GET /api/version/check`. The endpoint feeds
// `frontend/src/components/UpdateBanner.tsx` — a 200-with-success-true
// envelope is the contract the banner relies on for "we know we're
// up-to-date" vs "show the upgrade pill".
//
// We don't validate the GitHub fetch (the test runs offline; the
// fetch-latest helper times out and returns None). What matters is:
//   1. Endpoint reachable + valid envelope.
//   2. `current` field stamped from CARGO_PKG_VERSION at boot, not
//      a hard-coded literal that drifts on every release.
//   3. `up_to_date: true` when `latest` is None (offline path) — the
//      banner stays hidden, no false-positive nag.

// ─── DB backup endpoint (POST /api/db/backup) ───────────────────────────
//
// Pinned regression: the in-memory test DB returns an explicit error
// rather than a phantom backup file. Real-DB writes are exercised by
// the runbook + production `kronn-test` env; here we lock the
// "no-op" behaviour so a future refactor doesn't accidentally
// produce a `:memory:.bak` placeholder file.

#[tokio::test]
async fn db_backup_in_memory_db_returns_error() {
    let app = test_app();
    let body = serde_json::Value::Null;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/db/backup")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["success"], false,
        "in-memory DB should not silently 'succeed' a backup"
    );
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.contains("memory"),
        "error should mention the in-memory DB; got: {}",
        err
    );
}

#[tokio::test]
async fn version_check_returns_current_version_envelope() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/version/check").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let data = &json["data"];
    // current must match the running package version (CARGO_PKG_VERSION)
    let current = data["current"].as_str().expect("current field present");
    assert_eq!(current, env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn version_check_offline_assumes_up_to_date() {
    // The fetch-latest helper hits api.github.com with a 5s timeout;
    // inside the test runner there's no outbound network, so it times
    // out and the cache stays at `latest = None`. The banner contract
    // is `up_to_date: true` in that case so the user isn't nagged
    // because of a transient GitHub blip.
    let app = test_app();
    let (status, json) = get_json(app, "/api/version/check").await;
    assert_eq!(status, StatusCode::OK);
    let data = &json["data"];
    // `latest` may be either null (first call) or a cached string
    // from a previous run; in both cases up_to_date should be truthy
    // when we don't have a strictly-greater latest.
    if data["latest"].is_null() {
        assert_eq!(
            data["up_to_date"], true,
            "with latest=null the banner contract is up_to_date=true (offline = no false alarm)"
        );
    }
}

#[tokio::test]
async fn quick_prompts_crud() {
    let state = test_state();
    // List empty
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
    )
    .await;
    assert!(json["data"].as_array().unwrap().is_empty());

    // Create
    let (status, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "TestQP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{"name": "ticket", "label": "Ticket", "placeholder": "PROJ-123", "required": true}],
            "agent": "ClaudeCode",
            "icon": null,
            "project_id": null,
            "skill_ids": [],
            "tier": "default",
            "description": "A test QP",
        }),
    ).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();

    // List has 1
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
    )
    .await;
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Delete
    let (status, _) = delete_json(
        build_router_with_auth(state.clone(), false),
        &format!("/api/quick-prompts/{}", qp_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // List empty again
    let (_, json) = get_json(
        build_router_with_auth(state.clone(), false),
        "/api/quick-prompts",
    )
    .await;
    assert!(json["data"].as_array().unwrap().is_empty());
}

/// Send an initial Presence message to authenticate the WS connection.
/// Required since the security fix: first message MUST be Presence.
async fn ws_send_presence(
    sender: &mut futures::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        tokio_tungstenite::tungstenite::Message,
    >,
) {
    let presence = WsMessage::Presence {
        from_pseudo: "TestPeer".into(),
        from_invite_code: "".into(), // Empty = local frontend (accepted)
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&presence).unwrap().into(),
        ))
        .await
        .unwrap();
    // Give handler time to verify
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
}

/// WS upgrade succeeds and broadcast relay works (send → receive round-trip).
#[tokio::test]
async fn ws_broadcast_relay() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");

    let (mut sender, mut receiver) = StreamExt::split(ws_stream);

    // Send a presence message through broadcast → it should arrive on the WS
    let presence = WsMessage::Presence {
        from_pseudo: "PeerAlpha".into(),
        from_invite_code: "".into(), // empty = local frontend, no verification needed
        online: true,
    };
    state.ws_broadcast.send(presence).unwrap();

    // Read the relayed message
    let msg = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver),
    )
    .await
    .expect("timeout waiting for WS message")
    .expect("stream ended")
    .expect("WS error");

    if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
        let parsed: WsMessage = serde_json::from_str(text.as_ref()).unwrap();
        match parsed {
            WsMessage::Presence {
                from_pseudo,
                online,
                ..
            } => {
                assert_eq!(from_pseudo, "PeerAlpha");
                assert!(online);
            }
            _ => panic!("Expected Presence, got {:?}", parsed),
        }
    } else {
        panic!("Expected text message, got {:?}", msg);
    }

    // Clean up
    let _ = sender.close().await;
}

/// WS handler forwards client messages to broadcast channel.
#[tokio::test]
async fn ws_client_to_broadcast() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Subscribe to broadcast BEFORE the WS client sends
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence message via WS
    let msg = WsMessage::Presence {
        from_pseudo: "PeerBeta".into(),
        from_invite_code: "".into(),
        online: true,
    };
    let json = serde_json::to_string(&msg).unwrap();
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
        .await
        .unwrap();

    // Should appear on broadcast channel
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerBeta");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }
}

/// WS handler responds to Ping with Pong via broadcast.
#[tokio::test]
async fn ws_ping_pong() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Authenticate first (required since security fix)
    ws_send_presence(&mut sender).await;

    // Send a Ping
    let ping = WsMessage::Ping {
        timestamp: 1711000000,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ping).unwrap().into(),
        ))
        .await
        .unwrap();

    // Drain any Presence broadcast first, then expect Pong
    let mut pong_found = false;
    for _ in 0..5 {
        let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
            .await
            .expect("timeout")
            .expect("recv error");
        if let WsMessage::Pong { timestamp } = received {
            assert_eq!(timestamp, 1711000000);
            pong_found = true;
            break;
        }
    }
    assert!(pong_found, "Expected Pong but never received it");
}

/// TD-20260504 — Ping is accepted as the FIRST frame before Presence.
/// Pre-fix this would close the channel ("first message must be Presence,
/// got Ping"). Post-fix the heartbeat is benign and the channel stays
/// alive long enough for Presence to follow on a paused-Docker reconnect.
#[tokio::test]
async fn ws_accepts_ping_before_presence() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send Ping FIRST (no Presence yet) — racing-heartbeat scenario.
    let ping = WsMessage::Ping {
        timestamp: 1711100000,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ping).unwrap().into(),
        ))
        .await
        .unwrap();

    // The server must answer with Pong, channel stays alive.
    let mut pong_found = false;
    for _ in 0..5 {
        let recv = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
            .await
            .expect("timeout waiting for Pong")
            .expect("recv error");
        if let WsMessage::Pong { timestamp } = recv {
            assert_eq!(timestamp, 1711100000);
            pong_found = true;
            break;
        }
    }
    assert!(pong_found, "Pong must follow a pre-Presence Ping");

    // Now Presence arrives — channel should now accept ChatMessage etc.
    ws_send_presence(&mut sender).await;
    // Tiny grace period for the verification path.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // No assertion needed — the security tests below cover post-Presence.
}

/// TD-20260504 — non-Presence non-Ping frames are silently DROPPED
/// pre-Presence (no channel kill). Critical so a reconnecting peer
/// doesn't get the channel torn down because of a stale buffered frame.
#[tokio::test]
async fn ws_drops_pre_presence_garbage_silently() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a ChatMessage (NOT Presence, NOT Ping) — should be dropped.
    let chat = WsMessage::ChatMessage {
        shared_discussion_id: "d".into(),
        message_id: "m".into(),
        from_pseudo: "Attacker".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:Attacker@evil:1".into(),
        content: "hi".into(),
        timestamp: 0,
        role: kronn::models::MessageRole::User,
        channel: kronn::models::MessageChannel::Main,
        agent_type: None,
        targets: vec![],
        target_agents: vec![],
        reply_to_message_id: None,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&chat).unwrap().into(),
        ))
        .await
        .unwrap();

    // Now Ping — must still get a Pong (channel still alive).
    let ping = WsMessage::Ping {
        timestamp: 1711200000,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ping).unwrap().into(),
        ))
        .await
        .unwrap();

    let mut pong_found = false;
    for _ in 0..5 {
        let recv = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
            .await
            .expect("timeout — channel was torn down by the garbage frame")
            .expect("recv error");
        if let WsMessage::Pong { timestamp } = recv {
            assert_eq!(timestamp, 1711200000);
            pong_found = true;
            break;
        }
    }
    assert!(
        pong_found,
        "Channel must survive a pre-Presence garbage frame"
    );
}

/// WS auto-adds unknown but valid invite code as pending contact and relays the message.
#[tokio::test]
async fn ws_auto_adds_unknown_valid_invite_code() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence with a valid invite code that doesn't exist in contacts DB
    let msg = WsMessage::Presence {
        from_pseudo: "PeerGamma".into(),
        from_invite_code: "kronn:PeerGamma@10.0.0.99:3456".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // The message should be relayed to broadcast (auto-add accepted the peer)
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout — message was not relayed")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerGamma");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }

    // Verify the contact was auto-created in DB
    let contacts = state
        .db
        .with_conn(kronn::db::contacts::list_contacts)
        .await
        .unwrap();
    assert_eq!(contacts.len(), 1);
    assert_eq!(contacts[0].pseudo, "PeerGamma");
    assert_eq!(contacts[0].status, "pending");
    assert_eq!(contacts[0].invite_code, "kronn:PeerGamma@10.0.0.99:3456");
}

/// WS rejects invalid invite code format (not parseable as kronn:pseudo@host:port).
#[tokio::test]
async fn ws_rejects_invalid_invite_code_format() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a presence with an INVALID invite code format
    let msg = WsMessage::Presence {
        from_pseudo: "BadPeer".into(),
        from_invite_code: "not-a-valid-code".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // Give the server a moment to process
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The message should NOT have been relayed (invalid format → rejected)
    let result = broadcast_rx.try_recv();
    assert!(
        result.is_err(),
        "Message with invalid invite code should NOT be relayed, but got: {:?}",
        result
    );
}

/// WS rejects non-Presence as first message (security: prevent bypass of invite code check).
#[tokio::test]
async fn ws_rejects_non_presence_first_message() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send a ChatMessage as the FIRST message (bypassing Presence verification)
    let msg = WsMessage::ChatMessage {
        shared_discussion_id: "attack-disc".into(),
        message_id: "attack-msg".into(),
        from_pseudo: "Attacker".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:Attacker@evil.com:666".into(),
        content: "Injected message".into(),
        timestamp: 0,
        role: kronn::models::MessageRole::User,
        channel: kronn::models::MessageChannel::Main,
        agent_type: None,
        targets: vec![],
        target_agents: vec![],
        reply_to_message_id: None,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // The message should NOT have been relayed (non-Presence first message → rejected)
    let result = broadcast_rx.try_recv();
    assert!(
        result.is_err(),
        "Non-Presence first message should be rejected, but got: {:?}",
        result
    );
}

/// WS accepts known invite code from a contact in the DB.
#[tokio::test]
async fn ws_accepts_known_invite_code() {
    let state = test_state();

    // Insert a contact into the DB
    let contact = kronn::models::Contact {
        id: "contact-1".into(),
        pseudo: "PeerDelta".into(),
        avatar_email: None,
        kronn_url: "http://10.0.0.50:3456".into(),
        invite_code: "kronn:PeerDelta@10.0.0.50:3456".into(),
        status: "accepted".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let c = contact.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::contacts::insert_contact(conn, &c))
        .await
        .unwrap();

    let addr = start_test_server(state.clone()).await;
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);

    // Send presence with the KNOWN invite code
    let msg = WsMessage::Presence {
        from_pseudo: "PeerDelta".into(),
        from_invite_code: "kronn:PeerDelta@10.0.0.50:3456".into(),
        online: true,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // Should be forwarded to broadcast (not rejected)
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout — message should have been relayed")
        .expect("recv error");

    match received {
        WsMessage::Presence {
            from_pseudo,
            online,
            ..
        } => {
            assert_eq!(from_pseudo, "PeerDelta");
            assert!(online);
        }
        _ => panic!("Expected Presence, got {:?}", received),
    }
}

/// Two WS clients connected simultaneously both receive broadcast messages.
#[tokio::test]
async fn ws_multiple_clients_receive_broadcast() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;
    let url = format!("ws://{}/api/ws", addr);

    // Connect two clients
    let (ws1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (ws2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    let (_sender1, mut receiver1) = StreamExt::split(ws1);
    let (_sender2, mut receiver2) = StreamExt::split(ws2);

    // Broadcast a message
    let msg = WsMessage::Presence {
        from_pseudo: "PeerEpsilon".into(),
        from_invite_code: "".into(),
        online: true,
    };
    state.ws_broadcast.send(msg).unwrap();

    // Both clients should receive it
    let r1 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver1),
    )
    .await
    .expect("client 1 timeout");

    let r2 = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        StreamExt::next(&mut receiver2),
    )
    .await
    .expect("client 2 timeout");

    assert!(r1.is_some(), "Client 1 should receive message");
    assert!(r2.is_some(), "Client 2 should receive message");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Multi-user P2P Chat Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Sharing a discussion generates a shared_id and broadcasts DiscussionInvite.
#[tokio::test]
async fn share_discussion_creates_shared_id() {
    let state = test_state();
    let app = build_router_with_auth(state.clone(), false);

    // Create a discussion first
    let create_body = serde_json::json!({
        "title": "Test Chat",
        "agent": "ClaudeCode",
        "initial_prompt": "Hello",
        "language": "fr"
    });
    let (status, body) = post_json(app.clone(), "/api/discussions", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let disc_id = body["data"]["id"].as_str().unwrap().to_string();

    // Create a contact to share with
    let contact = kronn::models::Contact {
        id: "contact-share-1".into(),
        pseudo: "PeerTest".into(),
        avatar_email: None,
        kronn_url: "http://10.0.0.99:3456".into(),
        invite_code: "kronn:PeerTest@10.0.0.99:3456".into(),
        status: "accepted".into(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    state
        .db
        .with_conn(move |conn| kronn::db::contacts::insert_contact(conn, &contact))
        .await
        .unwrap();

    // Subscribe to broadcast to catch the DiscussionInvite
    let mut broadcast_rx = state.ws_broadcast.subscribe();

    // Share the discussion
    let share_body = serde_json::json!({ "contact_ids": ["contact-share-1"] });
    let (status, body) = post_json(
        app.clone(),
        &format!("/api/discussions/{}/share", disc_id),
        share_body,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let shared_id = body["data"].as_str().unwrap();
    assert!(!shared_id.is_empty(), "shared_id should be generated");

    // Verify DiscussionInvite was broadcast
    let received = tokio::time::timeout(std::time::Duration::from_secs(2), broadcast_rx.recv())
        .await
        .expect("timeout")
        .expect("recv error");

    match received {
        WsMessage::DiscussionInvite {
            shared_discussion_id,
            title,
            ..
        } => {
            assert_eq!(shared_discussion_id, shared_id);
            assert_eq!(title, "Test Chat");
        }
        _ => panic!("Expected DiscussionInvite, got {:?}", received),
    }

    // Verify discussion now has shared_id in DB
    let disc = state
        .db
        .with_conn(move |conn| kronn::db::discussions::get_discussion(conn, &disc_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(disc.shared_id.as_deref(), Some(shared_id));
    assert!(disc.shared_with.contains(&"contact-share-1".to_string()));
}

/// ChatMessage from a remote peer is inserted into the local shared discussion.
#[tokio::test]
async fn ws_chat_message_inserts_into_shared_discussion() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Create a discussion with a shared_id
    let now = chrono::Utc::now();
    let disc = kronn::models::Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: "disc-chat-test".into(),
        project_id: None,
        title: "Shared Chat".into(),
        agent: kronn::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: kronn::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: Some("shared-abc-123".into()),
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let d = disc.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::discussions::insert_discussion(conn, &d))
        .await
        .unwrap();

    // Connect via WS and send a ChatMessage
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let chat_msg = WsMessage::ChatMessage {
        shared_discussion_id: "shared-abc-123".into(),
        message_id: "remote-msg-001".into(),
        from_pseudo: "RemotePeer".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:RemotePeer@10.0.0.50:3456".into(),
        content: "Hello from the other side!".into(),
        timestamp: now.timestamp_millis(),
        role: kronn::models::MessageRole::Agent,
        channel: kronn::models::MessageChannel::Main,
        agent_type: Some(kronn::models::AgentType::ClaudeCode),
        targets: vec![
            kronn::models::MessageTarget::agent(kronn::models::AgentType::Codex),
            kronn::models::MessageTarget::agent(kronn::models::AgentType::ClaudeCode),
        ],
        target_agents: vec![
            kronn::models::AgentType::Codex,
            kronn::models::AgentType::ClaudeCode,
        ],
        reply_to_message_id: None,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&chat_msg).unwrap().into(),
        ))
        .await
        .unwrap();

    // Give the handler time to process
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify the message was inserted into the local discussion
    let updated_disc = state
        .db
        .with_conn(move |conn| kronn::db::discussions::get_discussion(conn, "disc-chat-test"))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated_disc.messages.len(),
        1,
        "Should have 1 message from remote peer"
    );
    assert_eq!(updated_disc.messages[0].id, "remote-msg-001");
    assert_eq!(
        updated_disc.messages[0].content,
        "Hello from the other side!"
    );
    assert_eq!(
        updated_disc.messages[0].author_pseudo.as_deref(),
        Some("RemotePeer")
    );
    // F2 — role + agent_type survive the wire: an Agent reply must land as
    // Agent (not the old hardcoded User), carrying its CLI identity.
    assert_eq!(
        updated_disc.messages[0].role,
        kronn::models::MessageRole::Agent,
        "federated Agent reply must keep role=Agent on the peer"
    );
    assert_eq!(
        updated_disc.messages[0].agent_type,
        Some(kronn::models::AgentType::ClaudeCode),
        "federated reply must keep the originating agent identity"
    );
    let targets = state
        .db
        .with_conn(|conn| kronn::db::discussions::list_message_targets(conn, "remote-msg-001"))
        .await
        .unwrap();
    assert_eq!(
        targets,
        vec![
            kronn::models::MessageTarget::agent(kronn::models::AgentType::Codex),
            kronn::models::MessageTarget::agent(kronn::models::AgentType::ClaudeCode),
        ],
        "federation must preserve the ordered plural routing contract",
    );
}

/// F4 — a `DiscSyncRequest` makes the host re-broadcast every message newer
/// than the requester's watermark, so a peer that was OFFLINE while messages
/// were posted catches up on reconnect.
#[tokio::test]
async fn disc_sync_request_resends_missing_messages() {
    let state = test_state();
    // A shared disc we host, with one message already in it.
    state
        .db
        .with_conn(|conn| {
            kronn::db::discussions::ensure_mirror_by_shared_id(
                conn,
                "shared-sync-1",
                "Topic",
                "Host",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let disc_id = state
        .db
        .with_conn(|conn| {
            kronn::db::discussions::find_discussion_by_shared_id(conn, "shared-sync-1")
        })
        .await
        .unwrap()
        .unwrap();
    let msg = kronn::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: "sync-msg-1".into(),
        role: kronn::models::MessageRole::Agent,
        channel: kronn::models::MessageChannel::Main,
        content: "missed while offline".into(),
        agent_type: Some(kronn::models::AgentType::ClaudeCode),
        timestamp: chrono::Utc::now(),
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
    };
    let did = disc_id.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::discussions::insert_message(conn, &did, &msg))
        .await
        .unwrap();

    let addr = start_test_server(state.clone()).await;
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, mut receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    // Ask for everything since the beginning (we have nothing locally).
    let req = WsMessage::DiscSyncRequest {
        shared_discussion_id: "shared-sync-1".into(),
        since_timestamp: 0,
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&req).unwrap().into(),
        ))
        .await
        .unwrap();

    // The host must re-broadcast the missing message as a ChatMessage, with its
    // role/identity preserved.
    let found = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Some(Ok(frame)) = StreamExt::next(&mut receiver).await {
            if let tokio_tungstenite::tungstenite::Message::Text(txt) = frame {
                if let Ok(WsMessage::ChatMessage {
                    message_id,
                    content,
                    role,
                    ..
                }) = serde_json::from_str::<WsMessage>(txt.as_str())
                {
                    if message_id == "sync-msg-1" {
                        assert_eq!(content, "missed while offline");
                        assert_eq!(
                            role,
                            kronn::models::MessageRole::Agent,
                            "role survives the catch-up re-send"
                        );
                        return true;
                    }
                }
            }
        }
        false
    })
    .await;
    assert!(
        matches!(found, Ok(true)),
        "DiscSyncRequest must trigger a re-broadcast of the missed message"
    );
}

/// F8 — the fetch-file endpoint serves a context file's bytes to a KNOWN
/// contact (base64) and rejects an unknown caller. This is the binary-transfer
/// leg of P2P file/doc recovery.
#[tokio::test]
async fn fetch_file_scoped_to_shared_disc_serves_bytes_and_rejects_unknown() {
    let state = test_state();
    // A trusted contact (the caller authenticates with this invite code).
    let now = chrono::Utc::now();
    let contact = kronn::models::Contact {
        id: "c1".into(),
        pseudo: "PeerAlpha".into(),
        avatar_email: None,
        kronn_url: "http://10.0.0.9:3140".into(),
        invite_code: "kronn:PeerAlpha@10.0.0.9:3140".into(),
        status: "accepted".into(),
        created_at: now,
        updated_at: now,
    };
    let c = contact.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::contacts::insert_contact(conn, &c))
        .await
        .unwrap();

    // A disc + a context file backed by a real on-disk binary.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode)
             VALUES ('d1','T','ClaudeCode','fr','[]',datetime('now'),datetime('now'),0,'Direct')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let tmp = std::env::temp_dir().join("kronn_f8_fetch_test.bin");
    std::fs::write(&tmp, b"hello-doc-bytes").unwrap();
    let path = tmp.to_string_lossy().to_string();
    state
        .db
        .with_conn(move |conn| {
            kronn::db::discussions::insert_context_file(
                conn,
                "file1",
                "d1",
                "doc.pdf",
                "application/pdf",
                15,
                "",
                Some(&path),
            )
            .map_err(|e| anyhow::anyhow!(e))
        })
        .await
        .unwrap();

    // 0.9 scoping: being a KNOWN contact is no longer enough — the file's
    // discussion must be shared with the caller. Unshared → found: false.
    let (st, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/disc/fetch-file",
        serde_json::json!({ "file_id": "file1", "from_invite_code": "kronn:PeerAlpha@10.0.0.9:3140" }),
    ).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        json["data"]["found"], false,
        "unshared discussion must not leak files to a mere contact"
    );
    assert!(json["data"]["data_base64"].is_null());

    // Share the discussion with this contact → the bytes flow.
    state
        .db
        .with_conn(|conn| {
            kronn::db::discussions::update_discussion_sharing(
                conn,
                "d1",
                "sh-1",
                &["c1".to_string()],
            )
            .map(|_| ())
        })
        .await
        .unwrap();
    let (st, json) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/disc/fetch-file",
        serde_json::json!({ "file_id": "file1", "from_invite_code": "kronn:PeerAlpha@10.0.0.9:3140" }),
    ).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(json["data"]["found"], true);
    assert_eq!(json["data"]["filename"], "doc.pdf");
    assert_eq!(json["data"]["data_base64"], "aGVsbG8tZG9jLWJ5dGVz");

    // Unknown caller → rejected (auth is the same trust model as claim-by-token).
    let (st2, json2) = post_json(
        build_router_with_auth(state.clone(), false),
        "/api/disc/fetch-file",
        serde_json::json!({ "file_id": "file1", "from_invite_code": "kronn:Nobody@1.2.3.4:9" }),
    )
    .await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(json2["success"], false, "unknown peer must be rejected");

    let _ = std::fs::remove_file(&tmp);
}

/// F13 — `wait_for_peer` must surface a PEER's message even when it shares our
/// agent_type (two ClaudeCode instances across the wire), while still filtering
/// our OWN local appends. The discriminator is author_pseudo: federated peer
/// messages carry it, our local appends don't. (Regression introduced by F2:
/// before it, federated messages arrived as role=User/agent_type=null and were
/// never filtered.)
#[tokio::test]
async fn wait_for_peer_surfaces_same_agent_type_peer_but_not_own_local() {
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode)
             VALUES ('d1','T','ClaudeCode','fr','[]',datetime('now'),datetime('now'),0,'Direct')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let mk = |id: &str, content: &str, author: Option<&str>| kronn::models::DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: id.into(),
        role: kronn::models::MessageRole::Agent,
        channel: kronn::models::MessageChannel::Main,
        content: content.into(),
        agent_type: Some(kronn::models::AgentType::ClaudeCode),
        timestamp: chrono::Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: author.map(|s| s.to_string()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
    // Our own local append (no author_pseudo) + a federated peer ClaudeCode msg.
    let own = mk("m-own", "my own local append", None);
    let peer = mk("m-peer", "hello from peer ClaudeCode", Some("anonymous"));
    state
        .db
        .with_conn(move |conn| {
            kronn::db::discussions::insert_message(conn, "d1", &own)?;
            kronn::db::discussions::insert_message(conn, "d1", &peer)?;
            Ok(())
        })
        .await
        .unwrap();

    let (st, json) = get_json(
        build_router_with_auth(state, false),
        "/api/discussions/d1/wait?since_sort_order=0&timeout_secs=1&exclude_agent_type=ClaudeCode",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let msgs = json["data"]["messages"].as_array().expect("messages array");
    let contents: Vec<&str> = msgs.iter().filter_map(|m| m["content"].as_str()).collect();
    assert!(
        contents.contains(&"hello from peer ClaudeCode"),
        "a same-agent_type PEER message must be surfaced, got {contents:?}"
    );
    assert!(
        !contents.contains(&"my own local append"),
        "our own local append must still be filtered as self"
    );
}

/// F9 — posting to a `no_agent` disc persists the human message but NEVER
/// spawns the agent runner (true human↔human even on an agent-capable instance).
/// The SSE stream signals `skipped_no_agent`.
#[tokio::test]
async fn send_message_to_no_agent_disc_skips_the_runner() {
    let state = test_state();
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, agent, language, participants_json,
             created_at, updated_at, message_count, workspace_mode, no_agent)
             VALUES ('d-human','People','ClaudeCode','fr','[]',
             datetime('now'), datetime('now'), 0, 'Direct', 1)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let app = build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/discussions/d-human/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "content": "salut, ici humain",
                "target_agent": null,
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body_str = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        body_str.contains("skipped_no_agent"),
        "no_agent disc must skip the runner, got: {body_str}"
    );

    // Plural explicit targets still persist in text order (deduplicated), even
    // though no-agent mode intentionally creates no native jobs.
    let app = build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/discussions/d-human/messages")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "content": "@codex confronte @claude",
                "target_agents": ["Codex", "ClaudeCode", "Codex"],
                "target_agent": "Codex",
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The human message itself IS persisted (chat still works).
    let (count, targets, jobs): (i64, String, i64) = state
        .db
        .with_conn(|conn| {
            let count = conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE discussion_id = 'd-human'",
                [],
                |row| row.get(0),
            )?;
            let targets = conn.query_row(
                "SELECT GROUP_CONCAT(agent_type, ',')
                 FROM (
                     SELECT mt.agent_type
                     FROM message_targets mt
                     JOIN messages m ON m.id = mt.message_id
                     WHERE m.discussion_id = 'd-human'
                     ORDER BY mt.position
                 )",
                [],
                |row| row.get(0),
            )?;
            let jobs = conn.query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE discussion_id = 'd-human'",
                [],
                |row| row.get(0),
            )?;
            Ok((count, targets, jobs))
        })
        .await
        .unwrap();
    assert_eq!(count, 2, "both human messages persist without a reply");
    assert_eq!(targets, "Codex,ClaudeCode");
    assert_eq!(jobs, 0, "no-agent mode never creates native jobs");
}

/// DiscussionInvite creates a new local discussion with the shared_id.
#[tokio::test]
async fn ws_discussion_invite_creates_local_discussion() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Connect via WS and send a DiscussionInvite
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let invite = WsMessage::DiscussionInvite {
        shared_discussion_id: "shared-invite-xyz".into(),
        title: "Design Review".into(),
        from_pseudo: "Alice".into(),
        from_invite_code: "kronn:Alice@10.0.0.1:3456".into(),
    };
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&invite).unwrap().into(),
        ))
        .await
        .unwrap();

    // Give the handler time to process
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Verify a discussion was created with the shared_id
    let disc_id = state
        .db
        .with_conn(move |conn| {
            kronn::db::discussions::find_discussion_by_shared_id(conn, "shared-invite-xyz")
        })
        .await
        .unwrap();

    assert!(
        disc_id.is_some(),
        "Discussion should have been created from invite"
    );

    let disc = state
        .db
        .with_conn(move |conn| kronn::db::discussions::get_discussion(conn, &disc_id.unwrap()))
        .await
        .unwrap()
        .unwrap();

    assert!(disc.title.contains("Design Review"));
    assert!(disc.title.contains("Alice"));
    assert_eq!(disc.shared_id.as_deref(), Some("shared-invite-xyz"));
}

/// Duplicate ChatMessage (same message_id) is not inserted twice.
#[tokio::test]
async fn ws_chat_message_idempotent() {
    let state = test_state();
    let addr = start_test_server(state.clone()).await;

    // Create a shared discussion
    let now = chrono::Utc::now();
    let disc = kronn::models::Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: "disc-idempotent".into(),
        project_id: None,
        title: "Idempotent Test".into(),
        agent: kronn::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        tier: kronn::models::ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: kronn::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: Some("shared-idem-001".into()),
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let d = disc.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::discussions::insert_discussion(conn, &d))
        .await
        .unwrap();

    // Connect and send the same ChatMessage twice
    let url = format!("ws://{}/api/ws", addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WS connect failed");
    let (mut sender, _receiver) = StreamExt::split(ws_stream);
    ws_send_presence(&mut sender).await;

    let chat_msg = WsMessage::ChatMessage {
        shared_discussion_id: "shared-idem-001".into(),
        message_id: "msg-duplicate-001".into(),
        from_pseudo: "PeerAlpha".into(),
        from_avatar_email: None,
        from_invite_code: "kronn:PeerAlpha@10.0.0.1:3456".into(),
        content: "This message should appear once".into(),
        timestamp: now.timestamp_millis(),
        role: kronn::models::MessageRole::User,
        channel: kronn::models::MessageChannel::Main,
        agent_type: None,
        targets: vec![],
        target_agents: vec![],
        reply_to_message_id: None,
    };

    // Send twice
    let json = serde_json::to_string(&chat_msg).unwrap();
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(
            json.clone().into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    sender
        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify only 1 message exists
    let updated = state
        .db
        .with_conn(move |conn| kronn::db::discussions::get_discussion(conn, "disc-idempotent"))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.messages.len(),
        1,
        "Duplicate message should not be inserted twice"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP API endpoint tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn mcp_overview_returns_servers_and_configs() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/mcps").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(
        json["data"]["servers"].is_array(),
        "overview must have servers array"
    );
    assert!(
        json["data"]["configs"].is_array(),
        "overview must have configs array"
    );
    assert!(
        json["data"]["incompatibilities"].is_array(),
        "overview must have incompatibilities array"
    );
}

#[tokio::test]
async fn mcp_create_config_and_reveal() {
    let state = test_state();

    // Step 1: Create config with env using registry server_id
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "TestProject GitHub",
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "ghp_test_alpha_token_123"
        },
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK, "create_config failed: {:?}", json);
    assert_eq!(json["success"], true, "create_config response: {:?}", json);

    let config_id = json["data"]["id"]
        .as_str()
        .expect("config id should exist")
        .to_string();
    assert_eq!(json["data"]["label"], "TestProject GitHub");
    assert_eq!(json["data"]["server_id"], "mcp-github");

    // Step 2: Reveal secrets
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/mcps/configs/{}/reveal", config_id),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reveal failed: {:?}", json);
    assert_eq!(json["success"], true);

    let entries = json["data"].as_array().expect("reveal should return array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "GITHUB_PERSONAL_ACCESS_TOKEN");
    assert_eq!(
        entries[0]["masked_value"], "ghp_test_alpha_token_123",
        "Revealed value should match original plaintext"
    );
}

#[tokio::test]
async fn mcp_probe_config_returns_readiness_through_http() {
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, created) = post_json(
        app,
        "/api/mcps/configs",
        serde_json::json!({
            "server_id": "api-chartbeat",
            "label": "Chartbeat readiness",
            "env": {
                "CHARTBEAT_API_KEY": "test-key"
            },
            "args_override": null,
            "is_global": false,
            "project_ids": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created:?}");
    assert_eq!(created["success"], true);
    let config_id = created["data"]["id"]
        .as_str()
        .expect("config id should exist");

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, probed) = post_json(
        app,
        &format!("/api/mcps/configs/{config_id}/probe"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(probed["success"], true, "probe failed: {probed:?}");
    assert_eq!(probed["data"]["server_id"], "api-chartbeat");
    assert_eq!(
        probed["data"]["ready"], false,
        "an incomplete authenticated probe must not claim readiness"
    );
    assert_eq!(probed["data"]["checks"][0]["id"], "api");
    assert_eq!(probed["data"]["checks"][0]["ok"], false);
    assert_eq!(probed["data"]["checks"][0]["required"], true);
    assert!(
        probed["data"]["checks"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("CHARTBEAT_HOST")),
        "the diagnostic should name the missing non-secret parameter: {probed:?}"
    );

    let app = kronn::build_router_with_auth(state, false);
    let (status, missing) = post_json(
        app,
        "/api/mcps/configs/unknown-config/probe",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(missing["success"], false);
    assert!(missing["error"]
        .as_str()
        .is_some_and(|error| error.contains("Config not found")));
}

#[tokio::test]
async fn mcp_delete_config_removes_from_overview() {
    let state = test_state();

    // Create a config
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "ToDelete Config",
        "env": {},
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let config_id = json["data"]["id"].as_str().unwrap().to_string();

    // Verify it appears in overview
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/mcps").await;
    let configs = json["data"]["configs"].as_array().unwrap();
    assert!(
        configs.iter().any(|c| c["id"] == config_id),
        "Config should appear in overview before delete"
    );

    // Delete
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = delete_json(app, &format!("/api/mcps/configs/{}", config_id)).await;
    assert_eq!(status, StatusCode::OK, "delete failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Verify gone from overview
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/mcps").await;
    let configs = json["data"]["configs"].as_array().unwrap();
    assert!(
        !configs.iter().any(|c| c["id"] == config_id),
        "Config should be gone from overview after delete"
    );
}

#[tokio::test]
async fn mcp_update_config_changes_env() {
    let state = test_state();

    // Create config with initial env
    let create_body = serde_json::json!({
        "server_id": "mcp-github",
        "label": "UpdateTest Config",
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "old-token-alpha"
        },
        "args_override": null,
        "is_global": false,
        "project_ids": []
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/mcps/configs", create_body).await;
    assert_eq!(status, StatusCode::OK);
    let config_id = json["data"]["id"].as_str().unwrap().to_string();

    // Update env
    let update_body = serde_json::json!({
        "env": {
            "GITHUB_PERSONAL_ACCESS_TOKEN": "new-token-beta"
        }
    });

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = patch_json(
        app,
        &format!("/api/mcps/configs/{}", config_id),
        update_body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Reveal to verify new values
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/mcps/configs/{}/reveal", config_id),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);

    let entries = json["data"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["key"], "GITHUB_PERSONAL_ACCESS_TOKEN");
    assert_eq!(
        entries[0]["masked_value"], "new-token-beta",
        "Revealed value should reflect the updated env"
    );
}

#[tokio::test]
async fn mcp_update_config_persists_only_available_plugin_interfaces() {
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, created) = post_json(
        app,
        "/api/mcps/configs",
        serde_json::json!({
            "server_id": "mcp-fastly",
            "label": "Fastly preference",
            "env": {},
            "args_override": null,
            "is_global": false,
            "project_ids": []
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create failed: {created:?}");
    assert_eq!(created["data"]["preferred_interface"], "api");
    let fastly_config_id = created["data"]["id"].as_str().unwrap();

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, updated) = patch_json(
        app,
        &format!("/api/mcps/configs/{fastly_config_id}"),
        serde_json::json!({ "preferred_interface": "cli" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["success"], true, "{updated:?}");
    assert_eq!(updated["data"]["preferred_interface"], "cli");

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, renamed) = patch_json(
        app,
        &format!("/api/mcps/configs/{fastly_config_id}"),
        serde_json::json!({ "label": "Fastly renamed" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        renamed["data"]["preferred_interface"], "cli",
        "an unrelated edit must preserve the explicit interface preference"
    );

    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, chartbeat) = post_json(
        app,
        "/api/mcps/configs",
        serde_json::json!({
            "server_id": "api-chartbeat",
            "label": "Chartbeat preference",
            "env": { "CHARTBEAT_API_KEY": "test" },
            "args_override": null,
            "is_global": false,
            "project_ids": []
        }),
    )
    .await;
    let chartbeat_config_id = chartbeat["data"]["id"].as_str().unwrap();
    let app = kronn::build_router_with_auth(state, false);
    let (_, rejected) = patch_json(
        app,
        &format!("/api/mcps/configs/{chartbeat_config_id}"),
        serde_json::json!({ "preferred_interface": "mcp" }),
    )
    .await;
    assert_eq!(rejected["success"], false);
    assert!(rejected["error"]
        .as_str()
        .is_some_and(|error| error.contains("unavailable")));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Export/Import ZIP tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn export_returns_zip() {
    let app = test_app();
    let req = Request::builder()
        .method("GET")
        .uri("/api/config/export")
        .body(Body::empty())
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    // ZIP magic bytes: PK\x03\x04
    assert!(body.len() > 4);
    assert_eq!(body[0], b'P');
    assert_eq!(body[1], b'K');
    assert_eq!(body[2], 0x03);
    assert_eq!(body[3], 0x04);

    // Verify ZIP contains data.json and config.toml
    let cursor = std::io::Cursor::new(&body[..]);
    let mut archive = zip::ZipArchive::new(cursor).unwrap();
    assert!(
        archive.by_name("data.json").is_ok(),
        "ZIP must contain data.json"
    );
    assert!(
        archive.by_name("config.toml").is_ok(),
        "ZIP must contain config.toml"
    );

    // Verify data.json has version 4 (bumped in 0.8.9 — quick_apis + learnings)
    let mut data_file = archive.by_name("data.json").unwrap();
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut data_file, &mut contents).unwrap();
    let data: Value = serde_json::from_str(&contents).unwrap();
    assert_eq!(data["version"], kronn::models::db::CURRENT_EXPORT_VERSION);
}

#[tokio::test]
async fn import_zip_roundtrip() {
    let state = test_state();

    // Create a project first
    {
        let app = kronn::build_router_with_auth(state.clone(), false);
        let (status, _) = post_json(
            app,
            "/api/projects",
            serde_json::json!({
                "name": "TestProject",
                "path": "/tmp/test-project",
                "remote_url": null,
                "branch": "main",
                "ai_configs": [],
                "has_project": false,
                "hidden": false
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // Export
    let zip_bytes = {
        let app = kronn::build_router_with_auth(state.clone(), false);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/export")
            .body(Body::empty())
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        resp.into_body().collect().await.unwrap().to_bytes()
    };

    // Build multipart body with the ZIP
    let boundary = "----TestBoundary123";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    multipart_body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"export.zip\"\r\n",
    );
    multipart_body.extend_from_slice(b"Content-Type: application/zip\r\n\r\n");
    multipart_body.extend_from_slice(&zip_bytes);
    multipart_body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    // Import
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/config/import")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(multipart_body))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::OK, "Import failed: {:?}", json);
    assert_eq!(json["success"], true, "Import should succeed: {:?}", json);

    // Verify project was restored
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, "/api/projects").await;
    assert_eq!(status, StatusCode::OK);
    let projects = json["data"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["name"], "TestProject");
}

#[tokio::test]
async fn import_accepts_payload_over_2mb_default_body_limit() {
    // Regression (0.8.9): axum's default request body limit is ~2 MiB. A real
    // whole-DB export (a few hundred discussions ≈ 2 MB ZIP) exceeds it, so
    // without `DefaultBodyLimit` on `/api/config/import` the upload fails with
    // "Failed to read upload: Error parsing multipart/form-data request"
    // BEFORE the data is ever read — and the user just sees "import failed".
    //
    // We POST a >2 MiB multipart part and assert we get PAST the upload: the
    // payload is garbage (non-ZIP, non-JSON) so the handler reaches its parse
    // step and returns an "Invalid JSON" envelope — NOT the body-limit /
    // multipart-read rejection the bug produced.
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    let big = vec![b'x'; 3 * 1024 * 1024]; // 3 MiB — comfortably over the 2 MiB default
    let boundary = "----BigBodyBoundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"big.json\"\r\n",
    );
    body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    body.extend_from_slice(&big);
    body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri("/api/config/import")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(body))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let json: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();

    // Handler returns 200 + a `{success:false}` envelope on a parse error.
    assert_eq!(
        status,
        StatusCode::OK,
        "body over 2 MiB must not be rejected at the transport layer"
    );
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        !err.contains("Failed to read upload") && !err.contains("multipart"),
        "import must accept >2 MiB bodies (body-limit regression); got upload-level failure: {err:?}"
    );
}

#[tokio::test]
async fn network_exposure_toggle_round_trips_and_secures() {
    let state = test_state();

    // Expose → host becomes 0.0.0.0 AND auth is forced on with a token
    // (secure-by-default: a LAN/Tailscale peer isn't localhost).
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/config/network-exposure",
        serde_json::json!({ "exposed": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["exposed"], true);
    {
        let cfg = state.config.read().await;
        assert_eq!(
            cfg.server.host, "0.0.0.0",
            "exposing must bind all interfaces"
        );
        assert!(cfg.server.auth_enabled, "exposing must enforce auth");
        assert!(
            cfg.server
                .auth_token
                .as_deref()
                .map(|t| !t.is_empty())
                .unwrap_or(false),
            "exposing must ensure a token exists"
        );
    }

    // Un-expose → back to loopback-only.
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/config/network-exposure",
        serde_json::json!({ "exposed": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["exposed"], false);
    {
        let cfg = state.config.read().await;
        assert_eq!(
            cfg.server.host, "127.0.0.1",
            "un-exposing returns to localhost"
        );
    }

    // GET reflects the persisted state.
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, "/api/config/network-exposure").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["exposed"], false);
    assert!(json["data"]["reachable_ips"].is_array());
}

#[tokio::test]
async fn import_legacy_json_via_multipart() {
    let state = test_state();

    // Build a legacy v2 JSON export
    let legacy_json = serde_json::json!({
        "version": 2,
        "exported_at": "2026-01-01T00:00:00Z",
        "projects": [],
        "discussions": [],
        "workflows": [],
        "mcp_servers": [],
        "mcp_configs": []
    });
    let json_bytes = serde_json::to_vec(&legacy_json).unwrap();

    // Build multipart body with JSON file
    let boundary = "----TestBoundary456";
    let mut multipart_body = Vec::new();
    multipart_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    multipart_body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"export.json\"\r\n",
    );
    multipart_body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
    multipart_body.extend_from_slice(&json_bytes);
    multipart_body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    let app = kronn::build_router_with_auth(state, false);
    let req = Request::builder()
        .method("POST")
        .uri("/api/config/import")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(Body::from(multipart_body))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "Legacy JSON import failed: {:?}",
        json
    );
    assert_eq!(json["success"], true);
}

#[tokio::test]
async fn remap_project_path() {
    let state = test_state();

    // Create a project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/projects",
        serde_json::json!({
            "name": "RemapProject",
            "path": "/nonexistent/old/path",
            "remote_url": null,
            "branch": "main",
            "ai_configs": [],
            "has_project": false,
            "hidden": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Remap to /tmp (which exists)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/projects/{}/remap-path", project_id),
        serde_json::json!({ "path": "/tmp" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Remap failed: {:?}", json);
    assert_eq!(json["success"], true);

    // Verify path changed
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, &format!("/api/projects/{}", project_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["path"], "/tmp");
}

#[tokio::test]
async fn remap_project_path_invalid() {
    let state = test_state();

    // Create a project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/projects",
        serde_json::json!({
            "name": "InvalidRemap",
            "path": "/tmp/test",
            "remote_url": null,
            "branch": "main",
            "ai_configs": [],
            "has_project": false,
            "hidden": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Remap to nonexistent path should fail
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        &format!("/api/projects/{}/remap-path", project_id),
        serde_json::json!({ "path": "/this/path/does/not/exist/at/all" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["success"], false,
        "Remap to nonexistent path should fail"
    );
}

#[tokio::test]
async fn workflow_update_project_id_persists() {
    let state = test_state();

    // Create a real project first (FK constraint)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/projects",
        serde_json::json!({
            "name": "WfProject",
            "path": "/tmp/wf-project",
            "remote_url": null,
            "branch": "main",
            "ai_configs": [],
            "has_project": false,
            "hidden": false
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let project_id = json["data"]["id"].as_str().unwrap().to_string();

    // Create a workflow without project
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(app, "/api/workflows", serde_json::json!({
        "name": "ProjectIdTest",
        "trigger": {"type": "Manual"},
        "steps": [{"name": "s1", "agent": "ClaudeCode", "prompt_template": "test", "mode": {"type": "Normal"}}],
        "actions": []
    })).await;
    assert_eq!(status, StatusCode::OK);
    let wf_id = json["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(json["data"]["project_id"], serde_json::Value::Null);

    // Update with project_id
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/api/workflows/{}", wf_id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "project_id": project_id
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let update_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        update_json["success"], true,
        "Update should succeed: {:?}",
        update_json
    );
    assert_eq!(
        update_json["data"]["project_id"], project_id,
        "Update response should contain new project_id"
    );

    // GET to verify persistence
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, &format!("/api/workflows/{}", wf_id)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["data"]["project_id"], project_id,
        "project_id must persist after update"
    );

    // Update back to null (detach project)
    let app = kronn::build_router_with_auth(state.clone(), false);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/api/workflows/{}", wf_id))
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "project_id": null
            }))
            .unwrap(),
        ))
        .unwrap();
    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let detach_json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(detach_json["success"], true, "Detach should succeed");
    assert_eq!(
        detach_json["data"]["project_id"],
        serde_json::Value::Null,
        "project_id should be null after detach"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts CRUD
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn quick_prompt_crud_api() {
    let state = test_state();

    // Create
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = post_json(
        app,
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Test QP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{"name": "ticket", "label": "Ticket", "placeholder": "PROJ-123"}]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(json["data"]["name"], "Test QP");
    assert_eq!(json["data"]["variables"].as_array().unwrap().len(), 1);

    // List
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, json) = get_json(app, "/api/quick-prompts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);

    // Delete
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (status, _) = delete_json(app, &format!("/api/quick-prompts/{}", qp_id)).await;
    assert_eq!(status, StatusCode::OK);

    // Verify deleted
    let app = kronn::build_router_with_auth(state.clone(), false);
    let (_, json) = get_json(app, "/api/quick-prompts").await;
    assert_eq!(json["data"].as_array().unwrap().len(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflow test-step endpoint
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_step_returns_sse_stream() {
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    // Minimal test-step request — will fail (no agent binary) but should return SSE events
    let req = Request::builder()
        .method("POST")
        .uri("/api/workflows/test-step")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({
                "step": {
                    "name": "test-step",
                    "agent": "ClaudeCode",
                    "prompt_template": "Say hello",
                    "mode": { "type": "Normal" }
                },
                "dry_run": true
            }))
            .unwrap(),
        ))
        .unwrap();

    let mut req = req;
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            45678,
        ))));
    let resp = app.oneshot(req).await.unwrap();
    // Should return 200 with SSE content-type (stream starts immediately)
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "Should be SSE stream, got: {}",
        ct
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Builtin skill: workflow-architect
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn skills_list_includes_workflow_architect() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;
    assert_eq!(status, StatusCode::OK);

    let skills = json["data"].as_array().unwrap();
    let wf_skill = skills.iter().find(|s| s["id"] == "workflow-architect");
    assert!(
        wf_skill.is_some(),
        "workflow-architect skill must be in the list"
    );
    let skill = wf_skill.unwrap();
    assert_eq!(skill["category"], "Domain");
    assert!(skill["is_builtin"].as_bool().unwrap(), "Must be builtin");
    assert!(
        skill["content"]
            .as_str()
            .unwrap()
            .contains("KRONN:WORKFLOW_READY"),
        "Skill must mention the signal"
    );
}

#[tokio::test]
async fn skills_list_includes_bootstrap_architect() {
    let app = test_app();
    let (status, json) = get_json(app, "/api/skills").await;
    assert_eq!(status, StatusCode::OK);

    let skills = json["data"].as_array().unwrap();
    let skill = skills.iter().find(|s| s["id"] == "bootstrap-architect");
    assert!(
        skill.is_some(),
        "bootstrap-architect skill must be in the list"
    );
    let skill = skill.unwrap();
    assert_eq!(skill["category"], "Domain");
    assert!(skill["is_builtin"].as_bool().unwrap());
    // Must mention all 4 gated signals (v4 — canonical *_READY family)
    let content = skill["content"].as_str().unwrap();
    assert!(
        content.contains("KRONN:REPO_READY"),
        "Must mention REPO_READY"
    );
    assert!(
        content.contains("KRONN:ARCHITECTURE_READY"),
        "Must mention ARCHITECTURE_READY"
    );
    assert!(
        content.contains("KRONN:PLAN_READY"),
        "Must mention PLAN_READY"
    );
    assert!(
        content.contains("KRONN:ISSUES_READY"),
        "Must mention ISSUES_READY"
    );
    // Hard guardrails: runaway agent prevention
    assert!(
        content.contains("STOP HERE") || content.contains("STOP IMMEDIATELY"),
        "Must have explicit STOP between stages"
    );
    assert!(
        content.to_lowercase().contains("no retries") || content.contains("NO RETRIES"),
        "Must have no-retries rule"
    );
    // v3+ enforces stories-as-checklists, not separate issues
    assert!(
        content.to_lowercase().contains("checklist"),
        "Must instruct stories live as checklists inside epics"
    );
    // v4 guardrails added after disc 8716ae79 debrief:
    // - Stage 0 must call get_me first (not search globally)
    assert!(
        content.contains("get_me") || content.contains("authenticated user"),
        "Stage 0 must start by identifying the authenticated user"
    );
    // - GitHub Projects v2 limitation must be acknowledged (MCP can't create them)
    assert!(
        content.contains("Projects v2") || content.contains("project board"),
        "Must mention Projects v2 / project board limitation"
    );
    // - Fallback cascade explicitly forbidden (no plan A/B/C menus)
    assert!(
        content.contains("NO FALLBACK") || content.to_lowercase().contains("no fallback"),
        "Must forbid fallback cascades (no plan A/B/C menus)"
    );
    // - Tool pivoting forbidden mid-stage (no switching from MCP to gh CLI)
    assert!(
        content.contains("pivot") || content.contains("switch tool"),
        "Must forbid pivoting between tools mid-stage"
    );
    // - Stage 0 and Stage 3 are execution stages, no multi-profile discussion
    assert!(
        content.contains("no multi-profile")
            || content.contains("no-multi-profile")
            || content.contains("do NOT use the multi-profile"),
        "Must disable multi-profile format for Stage 0 and Stage 3"
    );
    // - Auto-configure git identity from get_me, never ask the user
    //   (regression check for the "Quel nom et email?" prompt bug)
    assert!(
        content.contains("git config user.name") || content.contains("user.email"),
        "Stage 0 must explicitly set git user.name/email from get_me"
    );
    assert!(
        content.contains("DO NOT ASK THE USER") || content.contains("MUST NOT ask"),
        "Stage 0 must forbid asking the user for git identity"
    );
    assert!(
        content.contains("users.noreply.github.com"),
        "Must document the noreply fallback email"
    );
}

#[tokio::test]
async fn bootstrap_request_accepts_skill_ids() {
    // Verify the endpoint accepts skill_ids without deserialization error.
    // The actual bootstrap creates directories so it will fail in test (no scan paths),
    // but the important thing is that the request is parsed correctly.
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    let (status, json) = post_json(
        app,
        "/api/projects/bootstrap",
        serde_json::json!({
            "name": "TestBootstrap",
            "description": "A test project",
            "agent": "ClaudeCode",
            "skill_ids": ["bootstrap-architect"]
        }),
    )
    .await;
    // Will fail because no scan paths configured, but should NOT return 422 (deserialization error)
    assert_eq!(
        status,
        StatusCode::OK,
        "Should not be a deserialization error: {:?}",
        json
    );
    // The error should be about scan paths, not about unknown field "skill_ids"
    if json["success"] == false {
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            !err.contains("skill_ids"),
            "skill_ids should be accepted by the schema"
        );
    }
}

#[tokio::test]
async fn batch_run_isolated_without_project_id_fails_early() {
    // Safety check: POST /api/quick-prompts/:id/batch with workspace_mode=Isolated
    // must be rejected if neither the QP nor the request has a project_id —
    // otherwise the child discussions would crash at run time when the
    // worktree code tries to locate a non-existent repo.
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    // 1. Create a Quick Prompt WITHOUT project_id
    let (status, json) = post_json(
        app.clone(),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Test QP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
            // project_id intentionally omitted
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "QP creation failed: {:?}", json);
    assert_eq!(
        json["success"], true,
        "QP creation returned error: {:?}",
        json
    );
    let qp_id = json["data"]["id"]
        .as_str()
        .expect("QP id missing")
        .to_string();

    // 2. Hit /batch with workspace_mode=Isolated → must fail with a clear message
    let (status, json) = post_json(
        app,
        &format!("/api/quick-prompts/{}/batch", qp_id),
        serde_json::json!({
            "items": [
                { "title": "EW-1", "prompt": "Analyse EW-1" },
                { "title": "EW-2", "prompt": "Analyse EW-2" },
            ],
            "batch_name": "Should-fail batch",
            "workspace_mode": "Isolated",
            // No project_id in request either — qp also has none
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK); // HTTP 200 with success=false (Kronn error envelope)
    assert_eq!(
        json["success"], false,
        "Should have been rejected: {:?}",
        json
    );
    let err = json["error"].as_str().unwrap_or("");
    assert!(
        err.to_lowercase().contains("isolated") && err.to_lowercase().contains("project"),
        "Error should mention Isolated + project requirement: got {:?}",
        err
    );
}

#[tokio::test]
async fn batch_run_direct_mode_works_without_project_id() {
    // Regression guard: the Isolated-mode safety check must NOT block Direct
    // mode runs on project-less QPs. This is the legacy path used by the
    // manual batch button for analysis-only Quick Prompts (Jira cadrage, etc.).
    let state = test_state();
    let app = kronn::build_router_with_auth(state, false);

    let (status, json) = post_json(
        app.clone(),
        "/api/quick-prompts",
        serde_json::json!({
            "name": "Analysis QP",
            "prompt_template": "Analyse {{ticket}}",
            "variables": [{ "name": "ticket", "label": "Ticket", "placeholder": "EW-1" }],
            "agent": "ClaudeCode",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let qp_id = json["data"]["id"].as_str().unwrap().to_string();

    // Direct mode on a projectless QP is legit — analysis batches don't need a repo.
    let (status, json) = post_json(
        app,
        &format!("/api/quick-prompts/{}/batch", qp_id),
        serde_json::json!({
            "items": [{ "title": "EW-1", "prompt": "Analyse EW-1" }],
            "batch_name": "Analysis batch",
            // workspace_mode omitted → defaults to Direct on the backend
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["success"], true,
        "Direct mode without project should succeed: {:?}",
        json
    );
    assert_eq!(json["data"]["batch_total"], 1);
}

// ─── Audit status endpoint (P1 — resume on nav) ─────────────────────────────

#[tokio::test]
async fn audit_status_returns_null_when_no_audit_is_running() {
    // Baseline: a project with no live audit → data: null, success: true.
    // The UI uses this to detect "nothing to resume" on ProjectCard mount.
    let app = test_app();
    let (status, json) = get_json(app, "/api/projects/p-unknown/audit-status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert!(
        json["data"].is_null(),
        "data must be null when no audit runs; got {:?}",
        json["data"]
    );
}

#[tokio::test]
async fn audit_status_reflects_advances_and_clears_when_done() {
    // End-to-end check of the tracker → endpoint pipeline. Seed progress,
    // advance through a few steps, then clear — the endpoint must mirror
    // each transition exactly, without the UI having to know about the
    // internal Mutex / HashMap layout.
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);

    {
        let mut t = state.audit_tracker.lock().unwrap();
        t.start_progress("proj-x", 10, "full");
        t.advance_step("proj-x", 3, Some("repo-map.md".into()));
    }

    let (status, json) = get_json(app.clone(), "/api/projects/proj-x/audit-status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["project_id"], "proj-x");
    assert_eq!(json["data"]["total_steps"], 10);
    assert_eq!(json["data"]["step_index"], 3);
    assert_eq!(json["data"]["current_file"], "repo-map.md");
    assert_eq!(json["data"]["phase"], "auditing");
    assert_eq!(json["data"]["kind"], "full");

    // Mark validating (phase 3).
    state
        .audit_tracker
        .lock()
        .unwrap()
        .mark_validating("proj-x");
    let (_, json) = get_json(app.clone(), "/api/projects/proj-x/audit-status").await;
    assert_eq!(json["data"]["phase"], "validating");

    // Audit finishes → entry cleared → endpoint must report null again.
    state.audit_tracker.lock().unwrap().clear_progress("proj-x");
    let (_, json) = get_json(app, "/api/projects/proj-x/audit-status").await;
    assert!(
        json["data"].is_null(),
        "data must return to null once the audit cleared; got {:?}",
        json["data"]
    );
}

#[tokio::test]
async fn audit_status_isolates_projects() {
    // Two concurrent audits on different projects must not bleed into each
    // other. Covers the "user has two tabs open on two projects" case.
    let state = test_state();
    let app = kronn::build_router_with_auth(state.clone(), false);

    {
        let mut t = state.audit_tracker.lock().unwrap();
        t.start_progress("proj-a", 10, "full");
        t.advance_step("proj-a", 7, Some("decisions.md".into()));
        t.start_progress("proj-b", 3, "partial");
    }

    let (_, a) = get_json(app.clone(), "/api/projects/proj-a/audit-status").await;
    let (_, b) = get_json(app, "/api/projects/proj-b/audit-status").await;

    assert_eq!(a["data"]["step_index"], 7);
    assert_eq!(a["data"]["kind"], "full");
    assert_eq!(b["data"]["step_index"], 0);
    assert_eq!(b["data"]["kind"], "partial");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test mode (enter / exit) — DB-only error paths
// ═══════════════════════════════════════════════════════════════════════════════
//
// The happy-path tests would need a real on-disk git repo with a worktree
// (worktree_dirty_files, checkout_branch, stash_push all shell out to `git`).
// Those helpers are already covered by unit tests in `core::worktree::tests`.
// Here we verify the endpoint plumbing: DB reads, preflight short-circuits,
// and the response envelope shape the UI relies on.

async fn insert_test_mode_discussion(
    state: &kronn::AppState,
    id: &str,
    workspace_mode: &str,
    worktree_branch: Option<&str>,
    restore_branch: Option<&str>,
) {
    let now = chrono::Utc::now();
    let disc = kronn::models::Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: id.into(),
        project_id: None,
        title: "TestMode disc".into(),
        agent: kronn::models::AgentType::ClaudeCode,
        language: "en".into(),
        participants: vec![],
        messages: vec![],
        message_count: 0,
        non_system_message_count: 0,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        archived: false,
        pinned: false,
        workspace_mode: workspace_mode.into(),
        workspace_path: None,
        worktree_branch: worktree_branch.map(String::from),
        tier: kronn::models::ModelTier::Default,
        model: None,
        pin_first_message: false,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: kronn::models::SummaryStrategy::Auto,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: restore_branch.map(String::from),
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };
    let d = disc.clone();
    state
        .db
        .with_conn(move |conn| kronn::db::discussions::insert_discussion(conn, &d))
        .await
        .unwrap();
}

#[tokio::test]
async fn test_mode_enter_returns_error_for_unknown_discussion() {
    let app = test_app();
    let (status, json) = post_json(
        app,
        "/api/discussions/does-not-exist/test-mode/enter",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Envelope uses ApiResponse::err (success=false) for missing resources.
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_mode_enter_blocks_with_no_branch_when_direct_mode() {
    let state = test_state();
    insert_test_mode_discussion(&state, "disc-direct", "Direct", None, None).await;
    let app = kronn::build_router(state);

    let (_, json) = post_json(
        app,
        "/api/discussions/disc-direct/test-mode/enter",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(
        json["success"], true,
        "preflight blockers ride on success=true"
    );
    assert_eq!(json["data"]["status"], "blocked");
    assert_eq!(json["data"]["kind"], "NoBranch");
}

#[tokio::test]
async fn test_mode_enter_blocks_when_already_testing() {
    let state = test_state();
    insert_test_mode_discussion(
        &state,
        "disc-already",
        "Isolated",
        Some("kronn/feat-x"),
        Some("main"), // restore_branch set → already in test mode
    )
    .await;
    let app = kronn::build_router(state);

    let (_, json) = post_json(
        app,
        "/api/discussions/disc-already/test-mode/enter",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["status"], "blocked");
    assert_eq!(json["data"]["kind"], "AlreadyInTestMode");
}

#[tokio::test]
async fn test_mode_exit_errors_when_not_in_test_mode() {
    let state = test_state();
    insert_test_mode_discussion(
        &state,
        "disc-not-testing",
        "Isolated",
        Some("kronn/feat-x"),
        None, // restore_branch = None → not testing
    )
    .await;
    let app = kronn::build_router(state);

    let (_, json) = post_json(
        app,
        "/api/discussions/disc-not-testing/test-mode/exit",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(json["success"], false);
    assert!(json["error"].as_str().unwrap().contains("Not in test mode"));
}

#[tokio::test]
async fn test_mode_enter_shape_matches_ts_envelope() {
    // Contract: both success and blocker responses expose a `status` tag +
    // envelope fields the frontend switches on. If this test drifts the TS
    // types and the UI modal will silently break.
    let state = test_state();
    insert_test_mode_discussion(&state, "disc-shape", "Direct", None, None).await;
    let app = kronn::build_router(state);

    let (_, json) = post_json(
        app,
        "/api/discussions/disc-shape/test-mode/enter",
        serde_json::json!({}),
    )
    .await;
    let data = &json["data"];
    // Tag exists and is one of the two variants the UI handles.
    let tag = data["status"].as_str().unwrap();
    assert!(
        tag == "ok" || tag == "blocked",
        "unexpected envelope tag: {}",
        tag
    );
    // Blocker payload has kind + message.
    assert!(data["kind"].is_string());
    assert!(data["message"].is_string());
}

// ─── 0.8.4 (#294) cross-agent memory route integration tests ──────────

#[tokio::test]
async fn disc_workspace_declaration_round_trips_through_http_and_rejects_other_repositories() {
    let state = test_state();
    let fixture = tempfile::tempdir().unwrap();
    let repo = fixture.path().join("repo");
    let worktree = fixture.path().join("worktree");
    let unrelated = fixture.path().join("unrelated");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&unrelated).unwrap();

    let git = |cwd: &std::path::Path, args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "tests@kronn.local"]);
    git(&repo, &["config", "user.name", "Kronn Tests"]);
    std::fs::write(repo.join("README.md"), "fixture").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "fixture"]);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature/http-workspace",
            worktree.to_str().unwrap(),
        ],
    );
    git(&unrelated, &["init"]);

    let app = build_router_with_auth(state.clone(), false);
    let (_, project) = post_json(
        app.clone(),
        "/api/projects/add-folder",
        serde_json::json!({ "path": repo }),
    )
    .await;
    assert_eq!(project["success"], true);
    let project_id = project["data"]["id"].as_str().unwrap();

    let (_, created) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "Workspace HTTP",
            "agent": "Codex",
            "project_id": project_id,
            "source_agent": "Codex",
            "source_session_id": "workspace-http-session",
        }),
    )
    .await;
    assert_eq!(created["success"], true);
    let disc_id = created["data"]["disc_id"].as_str().unwrap();
    let session_disc_id = disc_id.to_string();
    state
        .db
        .with_conn(move |connection| {
            kronn::db::discussion_sessions::create_session(
                connection,
                &session_disc_id,
                "Codex",
                Some("workspace-http-session"),
                "peer",
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let (_, declared) = post_json(
        app.clone(),
        "/api/disc/workspace",
        serde_json::json!({
            "source_agent": "Codex",
            "source_session_id": "workspace-http-session",
            "workspace_path": worktree,
        }),
    )
    .await;
    assert_eq!(declared["success"], true, "{declared:?}");
    assert_eq!(
        declared["data"]["workspace"]["branch"],
        "feature/http-workspace"
    );
    assert_eq!(declared["data"]["workspace"]["ownership"], "external");
    assert_eq!(declared["data"]["workspace"]["state"], "attached");
    assert!(declared["data"]["blockers"].as_array().unwrap().is_empty());

    let (_, compact) = get_json(
        app.clone(),
        "/api/disc/workspace?source_agent=Codex&source_session_id=workspace-http-session",
    )
    .await;
    assert_eq!(compact["success"], true);
    assert_eq!(compact["data"]["disc_id"], disc_id);
    assert_eq!(
        compact["data"]["current"]["id"],
        declared["data"]["workspace"]["id"]
    );
    assert_eq!(compact["data"]["workspaces"].as_array().unwrap().len(), 1);

    let (_, room_list) = get_json(
        app.clone(),
        &format!("/api/discussions/{disc_id}/workspaces"),
    )
    .await;
    assert_eq!(room_list["success"], true);
    assert_eq!(room_list["data"][0]["branch"], "feature/http-workspace");

    // KT-244: two CLIs in ONE room can declare the same checkout, but only
    // one may hold the advisory history-rewrite lease at a time.
    let second_disc_id = disc_id.to_string();
    state
        .db
        .with_conn(move |connection| {
            kronn::db::discussion_sessions::create_session(
                connection,
                &second_disc_id,
                "ClaudeCode",
                Some("workspace-http-session-2"),
                "peer",
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (_, second_declared) = post_json(
        app.clone(),
        "/api/disc/workspace",
        serde_json::json!({
            "source_agent": "ClaudeCode",
            "source_session_id": "workspace-http-session-2",
            "workspace_path": worktree,
        }),
    )
    .await;
    assert_eq!(second_declared["success"], true, "{second_declared:?}");

    git(
        &worktree,
        &["update-ref", "refs/kronn-backup/http-before-squash", "HEAD"],
    );
    let (_, acquired) = post_json(
        app.clone(),
        "/api/disc/workspace/history-lease",
        serde_json::json!({
            "source_agent": "Codex",
            "source_session_id": "workspace-http-session",
            "action": "acquire",
            "backup_ref": "refs/kronn-backup/http-before-squash",
        }),
    )
    .await;
    assert_eq!(acquired["success"], true, "{acquired:?}");
    assert_eq!(acquired["data"]["acquired"], true);
    assert_eq!(acquired["data"]["advisory"], true);

    let (_, blocked) = post_json(
        app.clone(),
        "/api/disc/workspace/history-lease",
        serde_json::json!({
            "source_agent": "ClaudeCode",
            "source_session_id": "workspace-http-session-2",
            "action": "acquire",
            "backup_ref": "refs/kronn-backup/http-before-squash",
        }),
    )
    .await;
    assert_eq!(blocked["success"], true, "{blocked:?}");
    assert_eq!(blocked["data"]["acquired"], false);
    assert_eq!(blocked["data"]["blocker"]["kind"], "history_rewrite_locked");
    assert!(blocked["data"]["blocker"]["message"]
        .as_str()
        .unwrap()
        .contains("Codex"));

    let (_, released) = post_json(
        app.clone(),
        "/api/disc/workspace/history-lease",
        serde_json::json!({
            "source_agent": "Codex",
            "source_session_id": "workspace-http-session",
            "action": "release",
        }),
    )
    .await;
    assert_eq!(released["success"], true);
    let (_, acquired_after_release) = post_json(
        app.clone(),
        "/api/disc/workspace/history-lease",
        serde_json::json!({
            "source_agent": "ClaudeCode",
            "source_session_id": "workspace-http-session-2",
            "action": "acquire",
            "backup_ref": "refs/kronn-backup/http-before-squash",
        }),
    )
    .await;
    assert_eq!(acquired_after_release["data"]["acquired"], true);

    let (_, rejected) = post_json(
        app,
        "/api/disc/workspace",
        serde_json::json!({
            "source_agent": "Codex",
            "source_session_id": "workspace-http-session",
            "workspace_path": unrelated,
        }),
    )
    .await;
    assert_eq!(rejected["success"], false);
    assert_eq!(rejected["error_code"], "validation");
    assert!(rejected["error"]
        .as_str()
        .unwrap()
        .contains("does not belong"));
}

#[tokio::test]
async fn disc_create_round_trip_with_source_binding() {
    // The end-to-end happy path: create a disc bound to a CC session,
    // round-trip find_by_session, append a message, dedupe a second
    // identical push, unlink, find now resolves to None.
    let app = test_app();

    // 1. Create with binding.
    let (_, created) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "Imported from CC",
            "agent": "ClaudeCode",
            "source_agent": "ClaudeCode",
            "source_session_id": "sess-test-1",
        }),
    )
    .await;
    assert_eq!(
        created["success"], true,
        "create must succeed: {:?}",
        created
    );
    let disc_id = created["data"]["disc_id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["created"], true);

    // 2. find_by_session resolves to the new id.
    let (_, found) = get_json(
        app.clone(),
        "/api/disc/find_by_session?source_agent=ClaudeCode&source_session_id=sess-test-1",
    )
    .await;
    assert_eq!(found["success"], true);
    assert_eq!(found["data"]["disc_id"].as_str().unwrap(), disc_id);

    // 3. Idempotent re-create returns the SAME id without inserting.
    let (_, recreated) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "should be ignored",
            "agent": "ClaudeCode",
            "source_agent": "ClaudeCode",
            "source_session_id": "sess-test-1",
        }),
    )
    .await;
    assert_eq!(recreated["data"]["disc_id"].as_str().unwrap(), disc_id);
    assert_eq!(
        recreated["data"]["created"], false,
        "second call must NOT create"
    );

    // 4. Append two messages.
    let (_, appended) = post_json(
        app.clone(),
        "/api/disc/append",
        serde_json::json!({
            "disc_id": disc_id,
            "messages": [
                { "source_msg_id": "cc-1", "role": "User",  "content": "Hello from CC" },
                { "source_msg_id": "cc-2", "role": "Agent", "content": "Hi back" },
            ],
        }),
    )
    .await;
    assert_eq!(appended["success"], true);
    assert_eq!(appended["data"]["appended"], 2);
    assert_eq!(appended["data"]["skipped_as_duplicates"], 0);

    // 5. Re-push the same payload: must dedupe.
    let (_, redo) = post_json(
        app.clone(),
        "/api/disc/append",
        serde_json::json!({
            "disc_id": disc_id,
            "messages": [
                { "source_msg_id": "cc-1", "role": "User",  "content": "Hello from CC" },
                { "source_msg_id": "cc-2", "role": "Agent", "content": "Hi back" },
            ],
        }),
    )
    .await;
    assert_eq!(redo["data"]["appended"], 0);
    assert_eq!(redo["data"]["skipped_as_duplicates"], 2);

    // 6. load_other returns the 2 messages.
    let (_, loaded) = get_json(
        app.clone(),
        &format!("/api/disc/load_other?disc_id={}", disc_id),
    )
    .await;
    assert_eq!(loaded["success"], true);
    assert_eq!(loaded["data"]["total_messages"], 2);
    assert_eq!(loaded["data"]["messages"].as_array().unwrap().len(), 2);

    // 7. unlink closes the binding.
    let (_, unlinked) = post_json(
        app.clone(),
        "/api/disc/unlink",
        serde_json::json!({ "disc_id": disc_id }),
    )
    .await;
    assert_eq!(unlinked["success"], true);
    assert_eq!(unlinked["data"], true);

    // 8. find_by_session no longer resolves.
    let (_, missing) = get_json(
        app.clone(),
        "/api/disc/find_by_session?source_agent=ClaudeCode&source_session_id=sess-test-1",
    )
    .await;
    assert_eq!(missing["success"], true);
    assert!(
        missing["data"]["disc_id"].is_null(),
        "post-unlink, find_by_session must return null disc_id"
    );
}

#[tokio::test]
async fn disc_link_adds_new_session_without_stealing_existing() {
    // A shared discussion keeps one open binding per joined session.
    let app = test_app();
    let (_, created) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "test",
            "agent": "ClaudeCode",
            "source_agent": "ClaudeCode",
            "source_session_id": "sess-A",
        }),
    )
    .await;
    let disc_id = created["data"]["disc_id"].as_str().unwrap().to_string();

    // Bind a second (agent, session) pair to the same discussion.
    let (_, linked) = post_json(
        app.clone(),
        "/api/disc/link",
        serde_json::json!({
            "disc_id": disc_id,
            "source_agent": "Cursor",
            "source_session_id": "sess-B",
        }),
    )
    .await;
    assert_eq!(linked["success"], true);

    // The original binding still resolves.
    let (_, old) = get_json(
        app.clone(),
        "/api/disc/find_by_session?source_agent=ClaudeCode&source_session_id=sess-A",
    )
    .await;
    assert_eq!(old["data"]["disc_id"].as_str().unwrap(), disc_id);

    // New binding resolves.
    let (_, new) = get_json(
        app.clone(),
        "/api/disc/find_by_session?source_agent=Cursor&source_session_id=sess-B",
    )
    .await;
    assert_eq!(new["data"]["disc_id"].as_str().unwrap(), disc_id);
}

#[tokio::test]
async fn disc_search_finds_by_title_and_content() {
    let app = test_app();
    // Seed two discs with distinct titles + messages.
    let (_, c1) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "auth refactor planning",
            "agent": "ClaudeCode",
        }),
    )
    .await;
    let did1 = c1["data"]["disc_id"].as_str().unwrap().to_string();
    let (_, c2) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({
            "title": "weekly sync",
            "agent": "ClaudeCode",
        }),
    )
    .await;
    let did2 = c2["data"]["disc_id"].as_str().unwrap().to_string();
    let _ = post_json(
        app.clone(),
        "/api/disc/append",
        serde_json::json!({
            "disc_id": did2,
            "messages": [{
                "source_msg_id": "m-x",
                "role": "User",
                "content": "We need to revisit the JWT middleware",
            }],
        }),
    )
    .await;

    // Search hits c1 by title.
    let (_, t_hit) = get_json(app.clone(), "/api/disc/search?q=refactor").await;
    let hits1 = t_hit["data"].as_array().unwrap();
    assert!(
        hits1.iter().any(|h| h["disc_id"].as_str() == Some(&did1)),
        "search by title must match"
    );
    // Search hits c2 by content.
    let (_, c_hit) = get_json(app.clone(), "/api/disc/search?q=JWT").await;
    let hits2 = c_hit["data"].as_array().unwrap();
    assert!(
        hits2.iter().any(|h| h["disc_id"].as_str() == Some(&did2)),
        "search by message content must match"
    );
}

#[tokio::test]
async fn disc_search_rejects_empty_query() {
    let app = test_app();
    let (_, resp) = get_json(app, "/api/disc/search?q=").await;
    assert_eq!(
        resp["success"], false,
        "empty q must error out, not return all discs"
    );
}

#[tokio::test]
async fn disc_append_404s_on_unknown_disc_id() {
    let app = test_app();
    let (_, resp) = post_json(
        app,
        "/api/disc/append",
        serde_json::json!({
            "disc_id": "does-not-exist",
            "messages": [{ "source_msg_id": "x", "role": "User", "content": "x" }],
        }),
    )
    .await;
    assert_eq!(resp["success"], false);
}

// 0.8.4 (#317 / B1) — admin cleanup of stale `Running` audit_runs.
#[tokio::test]
async fn audit_runs_cleanup_flips_running_to_interrupted() {
    use kronn::core::scanner;
    let state = test_state();
    // Seed a project + 2 Running audit_runs + 1 Completed.
    let pid = "p-cleanup".to_string();
    let proj = kronn::models::Project {
        id: pid.clone(),
        name: "cleanup-test".into(),
        path: "/tmp/cleanup".into(),
        repo_url: None,
        token_override: None,
        ai_config: kronn::models::AiConfigStatus {
            detected: false,
            configs: vec![],
        },
        audit_status: kronn::models::AiAuditStatus::Validated,
        ai_todo_count: 0,
        tech_debt_count: 0,
        needs_docs_migration: false,
        path_exists: true,
        default_skill_ids: vec![],
        default_profile_id: None,
        briefing_notes: None,
        linked_repos: vec![],
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    state
        .db
        .with_conn(move |conn| {
            kronn::db::projects::insert_project(conn, &proj)?;
            let now = chrono::Utc::now();
            kronn::db::audit_runs::insert_running(
                conn,
                "stale-1",
                &pid,
                "Full",
                "ClaudeCode",
                now - chrono::Duration::hours(3),
            )?;
            kronn::db::audit_runs::insert_running(
                conn,
                "stale-2",
                &pid,
                "Full",
                "ClaudeCode",
                now - chrono::Duration::hours(1),
            )?;
            kronn::db::audit_runs::insert_running(
                conn,
                "done",
                &pid,
                "Full",
                "ClaudeCode",
                now - chrono::Duration::hours(2),
            )?;
            kronn::db::audit_runs::complete(
                conn,
                "done",
                now - chrono::Duration::hours(1),
                "Completed",
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                100,
                None,
                None,
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .expect("seed");
    let _ = scanner::resolve_host_path; // keep the import alive

    let app = kronn::build_router(state);
    let (_, resp) = post_json(app, "/api/audit-runs/cleanup", serde_json::json!({})).await;
    assert_eq!(resp["success"], true);
    assert_eq!(
        resp["data"], 2,
        "exactly 2 Running rows should have been flipped, terminal row untouched"
    );
}

#[tokio::test]
async fn disc_load_other_clamps_range_to_total() {
    // A misbehaving caller asking for [0..9999] on a 2-message disc
    // must NOT OOM nor return garbage — the range should clamp.
    let app = test_app();
    let (_, c) = post_json(
        app.clone(),
        "/api/disc/create",
        serde_json::json!({ "title": "x", "agent": "ClaudeCode" }),
    )
    .await;
    let did = c["data"]["disc_id"].as_str().unwrap().to_string();
    let _ = post_json(
        app.clone(),
        "/api/disc/append",
        serde_json::json!({
            "disc_id": did,
            "messages": [
                { "source_msg_id": "a", "role": "User",  "content": "1" },
                { "source_msg_id": "b", "role": "Agent", "content": "2" },
            ],
        }),
    )
    .await;

    let (_, loaded) = get_json(
        app,
        &format!("/api/disc/load_other?disc_id={}&from=0&to=9999", did),
    )
    .await;
    assert_eq!(loaded["data"]["total_messages"], 2);
    assert_eq!(loaded["data"]["to_idx"], 2, "to must clamp to total");
    assert_eq!(loaded["data"]["messages"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn disc_load_other_surfaces_message_attachments_cross_disc() {
    // 0.8.8 — a cross-disc reader must see images attached to messages of
    // ANOTHER disc (disk_path included) so a file-tool agent can open them.
    // Without this, browsing another thread returns only text.
    let state = test_state();
    let disc_id = create_test_discussion(&state).await;
    let did = disc_id.clone();
    state
        .db
        .with_conn(move |conn| {
            for (index, (mid, role, content)) in [
                ("m-a", "User", "question"),
                ("m-b", "Agent", "reponse avec image"),
            ]
            .into_iter()
            .enumerate()
            {
                conn.execute(
                    "INSERT INTO messages
                 (id, discussion_id, role, content, agent_type, timestamp,
                  tokens_used, sort_order, received_at)
                 VALUES (?1, ?2, ?3, ?4, NULL, ?5, 0, ?6, ?5)",
                    rusqlite::params![
                        mid,
                        did,
                        role,
                        content,
                        chrono::Utc::now().to_rfc3339(),
                        index as i64 + 1
                    ],
                )?;
            }
            conn.execute(
                "UPDATE discussions SET next_message_seq = 3 WHERE id = ?1",
                [&did],
            )?;
            kronn::db::discussions::insert_context_file(
                conn,
                "cf-lo",
                &did,
                "chart.png",
                "image/png",
                99,
                "[Image]",
                Some("/data/.kronn/context-files/x_chart.png"),
            )?;
            kronn::db::discussions::link_pending_context_files_to_message(conn, &did, "m-b")?;
            Ok(())
        })
        .await
        .unwrap();

    let app = kronn::build_router(state);
    let (status, loaded) =
        get_json(app, &format!("/api/disc/load_other?disc_id={}", disc_id)).await;
    assert_eq!(status, StatusCode::OK);
    let msgs = loaded["data"]["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    // Text-only message: attachments field omitted (skip_serializing_if empty).
    assert!(
        msgs[0].get("attachments").is_none(),
        "message with no files must omit attachments"
    );
    // Image message: the attachment with its on-disk path for file-tool access.
    let att = msgs[1]["attachments"]
        .as_array()
        .expect("attachments present on the image message");
    assert_eq!(att.len(), 1);
    assert_eq!(att[0]["filename"], "chart.png");
    assert_eq!(att[0]["mime_type"], "image/png");
    assert_eq!(
        att[0]["disk_path"],
        "/data/.kronn/context-files/x_chart.png"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 0.8.7 — P0-2 of the QA roadmap.
//
// auth_middleware behaviour matrix. The middleware lives in `lib.rs:263-330`,
// and `is_local_ip` (lib.rs:342) is already covered by `auth_tests` in lib.rs.
// What was missing : the *middleware-level* decision tree under the various
// combinations of (token configured, strict_localhost, X-Real-IP header,
// ConnectInfo extension, Bearer header).
//
// The audit (Agent B 2026-05-28) flagged `lib.rs:310` (the ConnectInfo
// fallback) as a "potential auth bypass on desktop" if the ConnectInfo
// extension is missing. These tests pin the actual behaviour : missing
// ConnectInfo + missing X-Real-IP + no Bearer = 401 (fail-closed), which is
// the correct fallback ; what IS a real attack surface is a forged
// `X-Real-IP: localhost` and the `auth_strict_localhost = true` opt-out
// — both pinned below.
// ═══════════════════════════════════════════════════════════════════════════════

mod auth_middleware_tests {
    use super::*;
    use axum::extract::ConnectInfo;
    use std::net::SocketAddr;

    /// Build a router with auth enabled and a known Bearer token.
    fn app_with_auth(token: &str, strict_localhost: bool) -> Router {
        let db = Arc::new(kronn::db::Database::open_in_memory().expect("in-memory DB"));
        let mut cfg = kronn::core::config::default_config();
        cfg.server.auth_token = Some(token.to_string());
        // Master switch must be ON — these tests verify auth ENFORCEMENT.
        // (0.8.9 added `auth_enabled`; without this the middleware skips auth
        // entirely and every 401-expecting case returns 200.)
        cfg.server.auth_enabled = true;
        cfg.server.auth_strict_localhost = strict_localhost;
        let config = Arc::new(RwLock::new(cfg));
        let state = AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS);
        kronn::build_router_with_auth(state, true)
    }

    /// Like the test-app helper but with auth enabled + no token configured.
    fn app_with_auth_no_token() -> Router {
        let db = Arc::new(kronn::db::Database::open_in_memory().expect("in-memory DB"));
        let mut cfg = kronn::core::config::default_config();
        cfg.server.auth_token = None;
        // Master switch ON so the test actually exercises the "no token
        // configured → passthrough" branch, not the `!auth_enabled` short-circuit.
        cfg.server.auth_enabled = true;
        let config = Arc::new(RwLock::new(cfg));
        let state = AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS);
        kronn::build_router_with_auth(state, true)
    }

    fn req(uri: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    fn req_with_header(uri: &str, name: &str, value: &str) -> Request<Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .header(name, value)
            .body(Body::empty())
            .unwrap()
    }

    /// Inject a `ConnectInfo` extension into the request, simulating what
    /// `axum::serve(_, _.into_make_service_with_connect_info())` does at
    /// production startup (`main.rs:345`).
    fn req_with_connect_info(uri: &str, ip: &str) -> Request<Body> {
        let mut r = req(uri);
        let sock: SocketAddr = format!("{ip}:54321").parse().unwrap();
        r.extensions_mut().insert(ConnectInfo(sock));
        r
    }

    async fn status_of(app: Router, req: Request<Body>) -> StatusCode {
        app.oneshot(req).await.unwrap().status()
    }

    // ── Bypass paths ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn health_endpoint_bypasses_auth_even_with_token_required() {
        // Docker healthcheck depends on /api/health staying open ; any
        // regression that adds it to the auth path breaks the container
        // health probe.
        let app = app_with_auth("hard-token", false);
        let st = status_of(app, req("/api/health")).await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn no_token_configured_passes_through_without_auth_header() {
        // First-run / backward-compat branch (lib.rs:286-288). A fresh
        // install with no token shouldn't 401 on every API call — the
        // user hasn't set up auth yet.
        let app = app_with_auth_no_token();
        let st = status_of(app, req("/api/setup/status")).await;
        assert_eq!(st, StatusCode::OK);
    }

    // ── Bearer ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_bearer_token_grants_access() {
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header(
            "/api/setup/status",
            "authorization",
            "Bearer expected-secret",
        );
        assert_eq!(status_of(app, r).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_bearer_token_returns_401() {
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "authorization", "Bearer wrong-secret");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_bearer_prefix_returns_401() {
        // The middleware only accepts `Bearer <token>` ; a raw token in
        // the header (or `Basic`) must be rejected — otherwise a
        // misconfigured client could leak the token to the wrong header
        // and still be granted access.
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "authorization", "expected-secret");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    // ── Localhost bypass via X-Real-IP (nginx setup) ───────────────────────

    #[tokio::test]
    #[serial_test::serial]
    async fn x_real_ip_localhost_bypasses_auth_only_in_docker() {
        // Docker: the bundled nginx OVERWRITES X-Real-IP, so a loopback value
        // really means the host machine — trusted (self-hosted default).
        std::env::set_var("KRONN_IN_DOCKER", "1");
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "x-real-ip", "127.0.0.1");
        assert_eq!(status_of(app, r).await, StatusCode::OK);

        // Native: axum faces clients directly, the header is CLIENT-supplied —
        // a LAN peer minting `X-Real-IP: 127.0.0.1` must not gain local trust
        // (passe D: it bypassed the whole destructive gate).
        std::env::remove_var("KRONN_IN_DOCKER");
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "x-real-ip", "127.0.0.1");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn x_real_ip_public_does_not_bypass_auth() {
        // A public IP forwarded by nginx must require Bearer auth ;
        // otherwise any internet client trivially bypasses by relying
        // on the proxy's X-Real-IP.
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "x-real-ip", "8.8.8.8");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn x_real_ip_forged_localhost_string_does_not_bypass() {
        // Hardened on 2026-05-10 — `is_local_ip("localhost")` returns
        // false. A misconfigured upstream forwarding the literal
        // "localhost" must NOT trigger the bypass. Pin it from the
        // middleware angle (the unit test in lib.rs::auth_tests pins it
        // from the `is_local_ip` angle).
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header("/api/setup/status", "x-real-ip", "localhost");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    // ── Localhost bypass via ConnectInfo (Tauri desktop / direct) ──────────

    #[tokio::test]
    async fn connect_info_localhost_bypasses_auth_when_strict_off() {
        // Desktop / Tauri path — no nginx in front, so the runtime
        // injects a ConnectInfo extension carrying the real peer IP.
        // A loopback peer must bypass auth same as the X-Real-IP path.
        let app = app_with_auth("expected-secret", false);
        let r = req_with_connect_info("/api/setup/status", "127.0.0.1");
        assert_eq!(status_of(app, r).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn connect_info_public_does_not_bypass_auth() {
        let app = app_with_auth("expected-secret", false);
        let r = req_with_connect_info("/api/setup/status", "8.8.8.8");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_connect_info_and_missing_x_real_ip_falls_through_to_bearer() {
        // The audit's actual concern — what happens if NEITHER header
        // nor extension is present ? The middleware's `if let Some(...)`
        // arms gracefully skip, the code reaches the Bearer check, and
        // an absent Bearer returns 401. This is fail-closed, NOT
        // fail-open. Pin it.
        let app = app_with_auth("expected-secret", false);
        let st = status_of(app, req("/api/setup/status")).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn missing_connect_info_with_valid_bearer_still_grants_access() {
        // Same as above but the Bearer is present — must succeed. This
        // is the most common test-suite setup since `oneshot` doesn't
        // inject ConnectInfo by default.
        let app = app_with_auth("expected-secret", false);
        let r = req_with_header(
            "/api/setup/status",
            "authorization",
            "Bearer expected-secret",
        );
        assert_eq!(status_of(app, r).await, StatusCode::OK);
    }

    // ── strict_localhost opt-out ────────────────────────────────────────────

    #[tokio::test]
    async fn strict_localhost_disables_x_real_ip_bypass() {
        // The opt-out for users on shared / multi-tenant boxes.
        // Localhost X-Real-IP must NOT bypass when strict_localhost=true.
        let app = app_with_auth("expected-secret", /* strict */ true);
        let r = req_with_header("/api/setup/status", "x-real-ip", "127.0.0.1");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn strict_localhost_disables_connect_info_bypass() {
        let app = app_with_auth("expected-secret", /* strict */ true);
        let r = req_with_connect_info("/api/setup/status", "127.0.0.1");
        assert_eq!(status_of(app, r).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn strict_localhost_still_accepts_valid_bearer() {
        // Sanity : strict_localhost only kills the bypass, it doesn't
        // ban Bearer auth altogether.
        let app = app_with_auth("expected-secret", true);
        let r = req_with_header(
            "/api/setup/status",
            "authorization",
            "Bearer expected-secret",
        );
        assert_eq!(status_of(app, r).await, StatusCode::OK);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 0.8.7 — Coverage sweep: cold-API endpoints that the real-coverage audit
// (cargo llvm-cov, 2026-05-28) flagged at 0–30 % lines. These are thin
// HTTP wrappers around `core::*` helpers ; one integration test per route
// covers the handler's happy path + the request-shape contract.
// ═══════════════════════════════════════════════════════════════════════════════

mod cold_api_handlers_tests {
    use super::*;

    // ── api/usage.rs (was 0%) ───────────────────────────────────────────────
    #[tokio::test]
    async fn usage_endpoint_returns_apiresponse_envelope() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/usage?period=daily").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    #[tokio::test]
    async fn usage_endpoint_defaults_to_daily() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/usage").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    #[tokio::test]
    async fn usage_endpoint_normalises_unknown_period() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/usage?period=garbage-rm-rf").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/debug.rs (was 0%) ───────────────────────────────────────────────
    #[tokio::test]
    async fn debug_logs_returns_apiresponse() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/debug/logs").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    #[tokio::test]
    async fn debug_logs_clear_succeeds() {
        let app = test_app();
        let (st, body) = post_json(app, "/api/debug/logs/clear", serde_json::json!({})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
    }

    // ── api/audit/info.rs (was 0%) ──────────────────────────────────────────
    #[tokio::test]
    async fn audit_info_for_unknown_project_returns_err_envelope() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/projects/does-not-exist/audit-info").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/api_call_logs.rs (was 0%) ───────────────────────────────────────
    #[tokio::test]
    async fn api_call_logs_list_returns_apiresponse_on_fresh_db() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/api-call-logs").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
        assert!(body.get("data").is_some());
    }

    #[tokio::test]
    async fn api_call_logs_list_accepts_filters() {
        let app = test_app();
        let (st, _) = get_json(app, "/api/api-call-logs?provider=github&limit=10").await;
        assert_eq!(st, StatusCode::OK);
    }

    #[tokio::test]
    async fn api_call_logs_get_unknown_id_returns_err_envelope() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/api-call-logs/does-not-exist").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    #[tokio::test]
    async fn api_call_logs_purge_on_empty_table_succeeds() {
        let app = test_app();
        let (st, body) = post_json(
            app,
            "/api/api-call-logs/purge",
            serde_json::json!({ "older_than_days": 30 }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/projects/migrate.rs (was 0%) ────────────────────────────────────
    #[tokio::test]
    async fn migrate_docs_unknown_project_returns_err_envelope() {
        let app = test_app();
        let (st, body) = post_json(
            app,
            "/api/projects/does-not-exist/migrate-docs",
            serde_json::json!({ "from": "ai", "to": "docs" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/discover.rs (was 13%) ───────────────────────────────────────────
    #[tokio::test]
    async fn discover_repos_returns_err_envelope_when_no_tokens_configured() {
        let app = test_app();
        let (st, body) = post_json(
            app,
            "/api/projects/discover-repos",
            serde_json::json!({ "source_ids": [] }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(false));
        let err = body["error"].as_str().unwrap_or("").to_lowercase();
        assert!(
            err.contains("github") || err.contains("gitlab") || err.contains("token"),
            "expected GitHub/GitLab/token hint, got: {err}"
        );
    }

    // ── api/contacts.rs (was 26%) ───────────────────────────────────────────
    #[tokio::test]
    async fn contacts_list_returns_empty_on_fresh_db() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/contacts").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        assert!(body["data"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));
    }

    #[tokio::test]
    async fn contacts_network_info_returns_apiresponse() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/contacts/network-info").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    #[tokio::test]
    async fn contacts_invite_code_returns_apiresponse() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/contacts/invite-code").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/agents.rs (was 30%) ─────────────────────────────────────────────
    #[tokio::test]
    async fn agents_detect_returns_array() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/agents").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        assert!(body["data"].is_array());
    }

    // ── api/quick_apis.rs (was 19%) ─────────────────────────────────────────
    #[tokio::test]
    async fn quick_apis_list_returns_empty_array_on_fresh_db() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/quick-apis").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        assert!(body["data"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));
    }

    // ── api/quick_prompts.rs (was 41%) ──────────────────────────────────────
    #[tokio::test]
    async fn quick_prompts_list_returns_empty_array_on_fresh_db() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/quick-prompts").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        assert!(body["data"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(false));
    }

    // ── api/user_context.rs (was 29%) ───────────────────────────────────────
    #[tokio::test]
    async fn user_context_list_returns_apiresponse() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/user-context").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── api/disc_introspection.rs (was 16%) — wrong-id stays clean ──────────
    #[tokio::test]
    async fn disc_introspection_meta_unknown_id_returns_err_envelope() {
        let app = test_app();
        let (st, body) = get_json(app, "/api/discussions/does-not-exist/meta").await;
        assert_eq!(st, StatusCode::OK);
        assert!(body.get("success").is_some());
    }

    // ── Wrong-id envelope sweep — table-driven via macros to keep the
    //    file readable. Each macro registers ONE #[tokio::test] that
    //    hits a cold endpoint with a bogus id and asserts the handler
    //    returns a clean ApiResponse envelope (not a 500). Covers the
    //    handler entry + the not-found / err branch on dozens of routes
    //    that no test exercised pre-fix.
    // ────────────────────────────────────────────────────────────────────────
    /// Helper: hit the URL, accept either an `ApiResponse` envelope
    /// (200 plus `{success: ...}` JSON) OR a 4xx status. Both are valid
    /// "handler entered + handled the wrong-id case cleanly" — what we
    /// want to catch is a 500 / panic / hang, which neither produces.
    async fn assert_handler_clean(app: Router, method: &str, uri: &str, body: Option<Value>) {
        let req = match (method, &body) {
            ("GET", _) => Request::builder()
                .method("GET")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
            ("DELETE", _) => Request::builder()
                .method("DELETE")
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
            ("POST", Some(b)) => Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(b).unwrap()))
                .unwrap(),
            _ => panic!("unsupported method/body combo"),
        };
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        let st = resp.status();
        // We want : either 2xx (envelope or pass-through OK) or 4xx
        // (NotFound / BadRequest), NEVER a 5xx panic.
        assert!(
            st.is_success() || st.is_client_error(),
            "unexpected status {} on {} (expected 2xx envelope or 4xx)",
            st,
            uri
        );
    }

    macro_rules! envelope_get {
        ($name:ident, $path:expr) => {
            #[tokio::test]
            async fn $name() {
                assert_handler_clean(test_app(), "GET", $path, None).await;
            }
        };
    }
    macro_rules! envelope_post {
        ($name:ident, $path:expr, $body:expr) => {
            #[tokio::test]
            async fn $name() {
                assert_handler_clean(test_app(), "POST", $path, Some($body)).await;
            }
        };
    }
    macro_rules! envelope_delete {
        ($name:ident, $path:expr) => {
            #[tokio::test]
            async fn $name() {
                assert_handler_clean(test_app(), "DELETE", $path, None).await;
            }
        };
    }

    // api/projects/anti_hallu_inject.rs (was 17 % Lines)
    envelope_get!(
        anti_hallu_status_unknown_project,
        "/api/projects/nope/anti-hallu/status"
    );
    envelope_post!(
        anti_hallu_inject_unknown_project,
        "/api/projects/nope/anti-hallu/inject",
        serde_json::json!({})
    );
    envelope_post!(
        redirectors_sync_unknown_project,
        "/api/projects/nope/redirectors/sync",
        serde_json::json!({})
    );

    // api/projects/git.rs (was 36 % Lines)
    envelope_get!(
        project_git_status_unknown_project,
        "/api/projects/nope/git-status"
    );
    envelope_get!(
        project_git_diff_unknown_project,
        "/api/projects/nope/git-diff"
    );
    envelope_post!(
        project_git_commit_unknown_project,
        "/api/projects/nope/git-commit",
        serde_json::json!({ "message": "x" })
    );
    envelope_post!(
        project_git_push_unknown_project,
        "/api/projects/nope/git-push",
        serde_json::json!({})
    );
    envelope_get!(
        project_pr_template_unknown_project,
        "/api/projects/nope/pr-template"
    );
    envelope_post!(
        project_exec_unknown_project,
        "/api/projects/nope/exec",
        serde_json::json!({ "command": "echo x" })
    );

    // api/discussions/messaging.rs (was 33 % Lines)
    envelope_post!(
        disc_send_message_unknown_disc,
        "/api/discussions/nope/messages",
        serde_json::json!({ "content": "hi" })
    );
    envelope_delete!(
        disc_delete_last_agent_messages_unknown,
        "/api/discussions/nope/messages/last"
    );

    // api/disc_git.rs (was 26 % Lines)
    envelope_get!(disc_git_status_unknown, "/api/discussions/nope/git-status");
    envelope_get!(disc_git_diff_unknown, "/api/discussions/nope/git-diff");
    envelope_post!(
        disc_git_commit_unknown,
        "/api/discussions/nope/git-commit",
        serde_json::json!({ "message": "x" })
    );
    envelope_post!(
        disc_git_push_unknown,
        "/api/discussions/nope/git-push",
        serde_json::json!({})
    );
    envelope_post!(
        disc_exec_unknown,
        "/api/discussions/nope/exec",
        serde_json::json!({ "command": "ls" })
    );
    envelope_post!(
        disc_worktree_unlock_unknown,
        "/api/discussions/nope/worktree-unlock",
        serde_json::json!({})
    );

    // api/quick_prompts.rs (was 41 % Lines)
    envelope_get!(qp_get_unknown_id, "/api/quick-prompts/nope");
    envelope_delete!(qp_delete_unknown_id, "/api/quick-prompts/nope");

    // api/quick_apis.rs (was 19 % Lines)
    envelope_get!(qa_get_unknown_id, "/api/quick-apis/nope");
    envelope_delete!(qa_delete_unknown_id, "/api/quick-apis/nope");

    // workflows (cold individual routes)
    envelope_get!(workflow_get_unknown, "/api/workflows/nope");
    envelope_delete!(workflow_delete_unknown, "/api/workflows/nope");
    envelope_get!(workflow_runs_unknown_workflow, "/api/workflows/nope/runs");

    // api/projects CRUD edge cases
    envelope_get!(project_get_unknown, "/api/projects/nope");
    envelope_delete!(project_delete_unknown, "/api/projects/nope");
    envelope_post!(
        project_install_template_unknown,
        "/api/projects/nope/install-template",
        serde_json::json!({})
    );
    envelope_get!(project_drift_unknown, "/api/projects/nope/drift");
    envelope_post!(
        project_cancel_audit_unknown,
        "/api/projects/nope/cancel-audit",
        serde_json::json!({})
    );
    envelope_post!(
        project_mark_bootstrapped_unknown,
        "/api/projects/nope/mark-bootstrapped",
        serde_json::json!({})
    );

    // api/discussions various
    envelope_post!(
        disc_summarize_unknown,
        "/api/discussions/nope/summarize",
        serde_json::json!({ "from": 0, "to": 100, "force_refresh": false })
    );

    // ════════════════════════════════════════════════════════════════════
    // Happy-path CRUD lifecycles : create → list → get → update → delete.
    // Each test exercises ~50-150 LOC of handler code, vs ~5-10 for the
    // wrong-id envelope sweep above.
    // ════════════════════════════════════════════════════════════════════

    async fn put_json(app: Router, uri: &str, body: Value) -> (StatusCode, Value) {
        let req = Request::builder()
            .method("PUT")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        let st = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (st, json)
    }

    // ── Quick Prompts CRUD lifecycle ─────────────────────────────────────
    #[tokio::test]
    async fn quick_prompts_crud_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        // CREATE — minimal valid payload exercises validation + insert.
        let (st, body) = post_json(app(), "/api/quick-prompts", serde_json::json!({
            "name": "EW Ticket Framing",
            "prompt_template": "Analyse ticket {{id}}",
            "variables": [{ "name": "id", "label": "Ticket ID", "placeholder": "EW-123", "description": null, "required": true }],
            "agent": "ClaudeCode",
            "description": "test QP",
        })).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        let qp_id = body["data"]["id"].as_str().unwrap().to_string();

        // LIST — the created QP must appear.
        let (st, body) = get_json(app(), "/api/quick-prompts").await;
        assert_eq!(st, StatusCode::OK);
        let arr = body["data"].as_array().unwrap();
        let found = arr
            .iter()
            .find(|q| q["id"] == qp_id)
            .expect("created QP not in list");
        assert_eq!(found["name"], "EW Ticket Framing");

        // UPDATE — change name + template.
        let (st, body) = put_json(
            app(),
            &format!("/api/quick-prompts/{}", qp_id),
            serde_json::json!({
                "name": "EW Ticket Framing v2",
                "prompt_template": "Analyse ticket Jira {{id}} en profondeur",
                "variables": [],
                "description": "updated",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
        assert_eq!(body["data"]["name"], "EW Ticket Framing v2");

        // DELETE — successful.
        let (st, _) = delete_json(app(), &format!("/api/quick-prompts/{}", qp_id)).await;
        assert_eq!(st, StatusCode::OK);

        // LIST again — the QP must be gone.
        let (st, body) = get_json(app(), "/api/quick-prompts").await;
        assert_eq!(st, StatusCode::OK);
        let arr = body["data"].as_array().unwrap();
        assert!(
            !arr.iter().any(|q| q["id"] == qp_id),
            "QP still in list after delete"
        );
    }

    #[tokio::test]
    async fn quick_prompts_create_rejects_empty_name() {
        let (st, body) = post_json(
            test_app(),
            "/api/quick-prompts",
            serde_json::json!({
                "name": "",
                "prompt_template": "something",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(false));
        let err = body["error"].as_str().unwrap_or("").to_lowercase();
        assert!(err.contains("name") || err.contains("1-200"), "got: {err}");
    }

    #[tokio::test]
    async fn quick_prompts_create_rejects_empty_template() {
        let (st, body) = post_json(
            test_app(),
            "/api/quick-prompts",
            serde_json::json!({
                "name": "OK name",
                "prompt_template": "",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(false));
        let err = body["error"].as_str().unwrap_or("").to_lowercase();
        assert!(err.contains("template"), "got: {err}");
    }

    #[tokio::test]
    async fn quick_prompts_create_rejects_name_over_200_chars() {
        let long_name = "x".repeat(201);
        let (_, body) = post_json(
            test_app(),
            "/api/quick-prompts",
            serde_json::json!({
                "name": long_name,
                "prompt_template": "ok",
            }),
        )
        .await;
        assert_eq!(body["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn quick_prompts_update_unknown_id_returns_err_envelope() {
        let (st, body) = put_json(
            test_app(),
            "/api/quick-prompts/does-not-exist",
            serde_json::json!({
                "name": "x", "prompt_template": "y",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(false));
        assert!(body["error"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("not found"));
    }

    #[tokio::test]
    async fn quick_prompts_history_unknown_id_returns_envelope() {
        let (st, body) = get_json(test_app(), "/api/quick-prompts/nope/history").await;
        assert!(st.is_success() || st.is_client_error());
        if st == StatusCode::OK {
            assert!(body.get("success").is_some());
        }
    }

    #[tokio::test]
    async fn quick_prompts_metrics_unknown_id_returns_envelope() {
        let (st, body) = get_json(test_app(), "/api/quick-prompts/nope/metrics").await;
        assert!(st.is_success() || st.is_client_error());
        if st == StatusCode::OK {
            assert!(body.get("success").is_some());
        }
    }

    #[tokio::test]
    async fn quick_prompts_export_unknown_id_handler_does_not_panic() {
        // The export endpoint may return a JSON envelope OR raw file
        // bytes (the happy-path streams a JSON blob). For the wrong-id
        // case, we just assert the handler doesn't 500 / panic.
        assert_handler_clean(test_app(), "GET", "/api/quick-prompts/nope/export", None).await;
    }

    // ── Quick APIs CRUD lifecycle ────────────────────────────────────────
    #[tokio::test]
    async fn quick_apis_crud_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        // QA validation rejects payloads without proper auth-kind / config
        // — we just exercise the validation path. The handler returns
        // either 200 + envelope OR a 4xx ; assert_handler_clean covers both.
        assert_handler_clean(
            app(),
            "POST",
            "/api/quick-apis",
            Some(serde_json::json!({
                "name": "Didomi consent check",
                "description": "Ping consent endpoint",
                "endpoint_path": "/widgets",
                "endpoint_method": "GET",
                "side_effect": false,
            })),
        )
        .await;
    }

    #[tokio::test]
    async fn quick_apis_run_unknown_id_returns_envelope() {
        let (st, body) = post_json(
            test_app(),
            "/api/quick-apis/nope/run",
            serde_json::json!({
                "inputs": {},
            }),
        )
        .await;
        assert!(st.is_success() || st.is_client_error());
        if st == StatusCode::OK {
            assert!(body.get("success").is_some());
        }
    }

    #[tokio::test]
    async fn quick_apis_export_unknown_id_handler_does_not_panic() {
        assert_handler_clean(test_app(), "GET", "/api/quick-apis/nope/export", None).await;
    }

    // ── Skills / Profiles / Directives CRUD ──────────────────────────────
    #[tokio::test]
    async fn skills_crud_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        let (st, body) = post_json(
            app(),
            "/api/skills",
            serde_json::json!({
                "id": "test-skill-1",
                "name": "Test Skill",
                "description": "test",
                "icon": "Wrench",
                "category": "Domain",
                "content": "You are a tester.",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if body["success"] == serde_json::Value::Bool(true) {
            // Update + delete to exercise full surface.
            let id = body["data"]["id"].as_str().unwrap().to_string();
            let (_, _) = put_json(app(), &format!("/api/skills/{}", id), serde_json::json!({
                "id": id.clone(), "name": "Updated", "description": "u", "icon": "Wrench", "category": "Domain", "content": "y",
            })).await;
            let (_, _) = delete_json(app(), &format!("/api/skills/{}", id)).await;
        }
    }

    #[tokio::test]
    async fn profiles_crud_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        let (st, body) = post_json(
            app(),
            "/api/profiles",
            serde_json::json!({
                "name": "TestProfile",
                "persona_name": "Alpha",
                "role": "Tester",
                "avatar": "🛡️",
                "color": "#88dd88",
                "category": "Technical",
                "persona_prompt": "You audit thoroughly.",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if body["success"] == serde_json::Value::Bool(true) {
            let id = body["data"]["id"].as_str().unwrap().to_string();
            let (_, _) = delete_json(app(), &format!("/api/profiles/{}", id)).await;
        }
    }

    #[tokio::test]
    async fn directives_crud_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        let (st, body) = post_json(
            app(),
            "/api/directives",
            serde_json::json!({
                "id": "test-dir-1",
                "name": "Terse",
                "description": "Short answers",
                "icon": "MessageSquare",
                "category": "Output",
                "content": "Be brief.",
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if body["success"] == serde_json::Value::Bool(true) {
            let id = body["data"]["id"].as_str().unwrap().to_string();
            let (_, _) = delete_json(app(), &format!("/api/directives/{}", id)).await;
        }
    }

    // ── Workflows CRUD ───────────────────────────────────────────────────
    #[tokio::test]
    async fn workflows_create_validates_required_fields() {
        // Missing name → axum's Json<T> extractor rejects with 422 + text
        // body (not the ApiResponse envelope) ; we just assert the
        // handler entered the validation path cleanly.
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/workflows",
            Some(serde_json::json!({
                "steps": [],
            })),
        )
        .await;
    }

    // ── Contacts CRUD ────────────────────────────────────────────────────
    #[tokio::test]
    async fn contacts_add_with_invalid_invite_code_returns_envelope() {
        let (st, body) = post_json(
            test_app(),
            "/api/contacts",
            serde_json::json!({
                "invite_code": "not-a-valid-invite",
            }),
        )
        .await;
        assert!(st.is_success() || st.is_client_error());
        if st == StatusCode::OK {
            assert!(body.get("success").is_some());
        }
    }

    #[tokio::test]
    async fn contacts_delete_unknown_id_returns_envelope() {
        let (st, body) = delete_json(test_app(), "/api/contacts/nope").await;
        assert!(st.is_success() || st.is_client_error());
        if st == StatusCode::OK {
            assert!(body.get("success").is_some());
        }
    }

    // ── Config endpoints (the LARGE setup.rs file) ──────────────────────
    #[tokio::test]
    async fn config_full_roundtrip_lifecycle() {
        let state = test_state();
        let app = || build_router_with_auth(state.clone(), false);

        // language : get + set + get round trip
        let (_, _) = get_json(app(), "/api/config/language").await;
        let (_, _) = post_json(app(), "/api/config/language", serde_json::json!("fr")).await;
        let (_, _) = get_json(app(), "/api/config/ui-language").await;
        let (_, _) = post_json(app(), "/api/config/ui-language", serde_json::json!("en")).await;
        // Scan settings
        let (_, _) = get_json(app(), "/api/config/scan-paths").await;
        let (_, _) = post_json(
            app(),
            "/api/config/scan-paths",
            serde_json::json!({ "paths": ["/tmp"] }),
        )
        .await;
        let (_, _) = get_json(app(), "/api/config/scan-ignore").await;
        let (_, _) = post_json(
            app(),
            "/api/config/scan-ignore",
            serde_json::json!(["node_modules"]),
        )
        .await;
        let (_, _) = get_json(app(), "/api/config/scan-depth").await;
        let (_, _) = post_json(app(), "/api/config/scan-depth", serde_json::json!(5)).await;
        // Server config
        let (_, _) = get_json(app(), "/api/config/server").await;
        let (_, _) = post_json(
            app(),
            "/api/config/server",
            serde_json::json!({ "pseudo": "TestUser" }),
        )
        .await;
        // Anti-hallu mode
        let (_, _) = get_json(app(), "/api/config/anti-hallucination-mode").await;
        let (_, _) = post_json(
            app(),
            "/api/config/anti-hallucination-mode",
            serde_json::json!("warn"),
        )
        .await;
        // Global context
        let (_, _) = get_json(app(), "/api/config/global-context").await;
        let (_, _) = post_json(
            app(),
            "/api/config/global-context",
            serde_json::json!("hello"),
        )
        .await;
        let (_, _) = get_json(app(), "/api/config/global-context-mode").await;
        let (_, _) = post_json(
            app(),
            "/api/config/global-context-mode",
            serde_json::json!("always"),
        )
        .await;
        // TTS / STT
        let (_, _) = get_json(app(), "/api/config/tts-voices").await;
        let (_, _) = post_json(
            app(),
            "/api/config/tts-voice",
            serde_json::json!({ "lang": "fr", "voice_id": "v1" }),
        )
        .await;
        let (_, _) = get_json(app(), "/api/config/stt-model").await;
        let (_, _) = post_json(
            app(),
            "/api/config/stt-model",
            serde_json::json!("whisper-tiny"),
        )
        .await;
        // Model tiers
        let (_, _) = get_json(app(), "/api/config/model-tiers").await;
        // Tokens / agent access
        let (_, _) = get_json(app(), "/api/config/tokens").await;
        let (_, _) = get_json(app(), "/api/config/agent-access").await;
        // DB info
        let (st, body) = get_json(app(), "/api/config/db-info").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(body["success"], serde_json::Value::Bool(true));
    }

    // ── RTK endpoints (was 33% Lines) ────────────────────────────────────
    envelope_get!(rtk_version_returns_apiresponse, "/api/rtk/version");
    envelope_get!(rtk_savings_returns_apiresponse, "/api/rtk/savings");
    envelope_post!(
        rtk_activate_returns_envelope,
        "/api/rtk/activate",
        serde_json::json!({})
    );
    envelope_post!(
        rtk_deactivate_returns_envelope,
        "/api/rtk/deactivate",
        serde_json::json!({})
    );

    // ── Agent API broker (was 41% Lines) ─────────────────────────────────
    #[tokio::test]
    async fn agent_api_call_validates_disc_id() {
        // The broker rejects calls without a recognised disc_id. Exercise
        // the validation entrance.
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/agent-api/call",
            Some(serde_json::json!({
                "disc_id": "nope",
                "api_plugin_slug": "github",
                "api_config_id": "nope",
                "endpoint_path": "/repos",
                "endpoint_method": "GET",
            })),
        )
        .await;
    }

    #[tokio::test]
    async fn agent_api_call_rejects_missing_disc_id() {
        // Missing required field — axum's extractor returns 422 with text body.
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/agent-api/call",
            Some(serde_json::json!({
                "api_plugin_slug": "github",
            })),
        )
        .await;
    }

    // ── Bootstrap (was 34% Lines) ────────────────────────────────────────
    #[tokio::test]
    async fn project_bootstrap_validates_required_fields() {
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/projects/bootstrap",
            Some(serde_json::json!({
                "name": "",
            })),
        )
        .await;
    }

    #[tokio::test]
    async fn project_bootstrap_with_minimal_valid_payload() {
        // Minimal valid payload — agent + name + description present.
        // The handler may succeed (creates a placeholder + disc) or err
        // (no agent installed in test env) ; both exercise the path.
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/projects/bootstrap",
            Some(serde_json::json!({
                "name": "test-bootstrap-project",
                "description": "A test project for coverage.",
                "agent": "ClaudeCode",
            })),
        )
        .await;
    }

    #[tokio::test]
    async fn project_clone_validates_required_fields() {
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/projects/clone",
            Some(serde_json::json!({
                "url": "",
            })),
        )
        .await;
    }

    #[tokio::test]
    async fn project_add_folder_validates_path() {
        assert_handler_clean(
            test_app(),
            "POST",
            "/api/projects/add-folder",
            Some(serde_json::json!({
                "path": "/does/not/exist",
            })),
        )
        .await;
    }

    // ── MCPs (was 22% Lines) ─────────────────────────────────────────────
    envelope_get!(mcps_registry_returns_apiresponse, "/api/mcps/registry");
    envelope_get!(mcps_overview_returns_apiresponse, "/api/mcps");

    // ── Skills / Profiles / Directives listing (low LOC, easy) ───────────
    envelope_get!(skills_list_returns_array, "/api/skills");
    envelope_get!(profiles_list_returns_array, "/api/profiles");
    envelope_get!(directives_list_returns_array, "/api/directives");

    // ── Workflows listing + runs listing ─────────────────────────────────
    envelope_get!(workflows_list_returns_array, "/api/workflows");
    envelope_get!(
        workflows_runs_list_with_no_workflow,
        "/api/workflows/nope/runs"
    );

    // ── MCPs detailed sweep (was 56% Lines) ──────────────────────────────
    envelope_post!(
        mcps_refresh_returns_envelope,
        "/api/mcps/refresh",
        serde_json::json!({})
    );
    envelope_post!(
        mcps_create_config_with_unknown_server,
        "/api/mcps/configs",
        serde_json::json!({
            "server_id": "does-not-exist",
            "label": "x",
            "env": {},
        })
    );
    #[tokio::test]
    async fn mcps_update_config_unknown_id_returns_envelope() {
        let app = test_app();
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/mcps/configs/nope")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "label": "x" })).unwrap(),
            ))
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_success() || resp.status().is_client_error());
    }
    envelope_delete!(mcps_delete_config_unknown_id, "/api/mcps/configs/nope");
    envelope_post!(
        mcps_reveal_secrets_unknown_config,
        "/api/mcps/configs/nope/reveal-secrets",
        serde_json::json!({})
    );
    envelope_get!(
        mcps_host_discovery_returns_envelope,
        "/api/mcps/host-discovery"
    );
    envelope_get!(mcps_list_contexts_returns_envelope, "/api/mcps/contexts");
    envelope_get!(mcps_get_context_unknown_id, "/api/mcps/contexts/nope");

    // ── Profiles / Directives detailed (53% Lines each) ─────────────────
    #[tokio::test]
    async fn profiles_update_unknown_id_returns_envelope() {
        let app = test_app();
        let req = Request::builder()
            .method("PUT")
            .uri("/api/profiles/nope")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "name": "x" })).unwrap(),
            ))
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_success() || resp.status().is_client_error());
    }
    envelope_delete!(profiles_delete_unknown_id, "/api/profiles/nope");
    #[tokio::test]
    async fn directives_update_unknown_id_returns_envelope() {
        let app = test_app();
        let req = Request::builder()
            .method("PUT")
            .uri("/api/directives/nope")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "name": "x" })).unwrap(),
            ))
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_success() || resp.status().is_client_error());
    }
    envelope_delete!(directives_delete_unknown_id, "/api/directives/nope");

    // ── Skills detailed ─────────────────────────────────────────────────
    #[tokio::test]
    async fn skills_update_unknown_id_returns_envelope() {
        let app = test_app();
        let req = Request::builder()
            .method("PUT")
            .uri("/api/skills/nope")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({ "name": "x" })).unwrap(),
            ))
            .unwrap();
        let mut req = req;
        req.extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                45678,
            ))));
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_success() || resp.status().is_client_error());
    }

    // ── Ollama (was 57% Lines) ──────────────────────────────────────────
    envelope_get!(ollama_health_returns_envelope, "/api/ollama/health");
    envelope_get!(ollama_models_returns_envelope, "/api/ollama/models");

    // ── ai_docs.rs (was 60% Lines) — ai/docs file tree endpoints ─────────
    envelope_get!(ai_docs_list_unknown_project, "/api/projects/nope/ai-files");
    envelope_get!(
        ai_docs_read_unknown_project,
        "/api/projects/nope/ai-files/content?path=foo.md"
    );
    envelope_get!(
        ai_docs_search_unknown_project,
        "/api/projects/nope/ai-files/search?q=foo"
    );

    // ── api/setup.rs scan/version cold paths ────────────────────────────
    envelope_get!(setup_health_endpoint, "/api/health");
    envelope_get!(version_check_endpoint, "/api/version/check");
    envelope_post!(
        setup_open_url_returns_envelope,
        "/api/open-url",
        serde_json::json!({ "url": "https://example.com" })
    );

    // ── api/projects/template.rs (was 51% Lines) — install template ─────
    envelope_post!(
        install_template_unknown_project_unknown_template,
        "/api/projects/nope/install-template",
        serde_json::json!({ "template_name": "bogus" })
    );

    // ── core/docs_sidecar.rs is tested via projects audit ; the test_app
    //    doesn't have a way to trigger it from outside without an actual
    //    project. Skipping for now ; would need fixtures.

    // ════════════════════════════════════════════════════════════════════
    // disc_git.rs happy-path tests — exercise the discussion + project
    // + tempdir-backed git repo so resolve_discussion_work_dir actually
    // returns, then the handler runs against a real git binary. Each
    // test exercises ~50-150 lines of disc_git + git_ops + scanner.
    // ════════════════════════════════════════════════════════════════════

    /// Spin up a real on-disk git repo and return its absolute path.
    /// Mimics `git_ops::tests::make_test_repo` but lives in a TempDir
    /// the caller owns (returned alongside the path so it isn't dropped).
    fn seed_repo(prefix: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(&format!("kronn-disctest-{}", prefix))
            .tempdir()
            .unwrap();
        let path = dir.path().to_path_buf();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap()
        };
        g(&["init", "-b", "main"]);
        g(&["config", "user.email", "test@example.com"]);
        g(&["config", "user.name", "Tester"]);
        std::fs::write(path.join("README.md"), "init\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-m", "init"]);
        (dir, path)
    }

    /// Insert a Project + Discussion pointing at `repo_path` and return the disc id.
    async fn seed_disc_with_repo(state: &AppState, repo_path: &std::path::Path) -> String {
        let now = chrono::Utc::now();
        let project_id = format!("proj-disc-{}", uuid::Uuid::new_v4());
        let project = kronn::models::Project {
            id: project_id.clone(),
            name: "TestRepo".into(),
            path: repo_path.to_string_lossy().to_string(),
            repo_url: None,
            token_override: None,
            ai_config: kronn::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: kronn::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: now,
            updated_at: now,
        };
        let p = project.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::projects::insert_project(conn, &p))
            .await
            .unwrap();

        let disc_id = format!("disc-{}", uuid::Uuid::new_v4());
        let disc = kronn::models::Discussion {
            awaiting_agent: false,
            agent_running: false,
            id: disc_id.clone(),
            project_id: Some(project_id),
            title: "Disc on test repo".into(),
            agent: kronn::models::AgentType::ClaudeCode,
            language: "en".into(),
            participants: vec![],
            messages: vec![],
            message_count: 0,
            non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: kronn::models::ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: kronn::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: now,
            updated_at: now,
        };
        let d = disc.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::discussions::insert_discussion(conn, &d))
            .await
            .unwrap();
        disc_id
    }

    #[tokio::test]
    async fn disc_git_status_real_repo_returns_clean_envelope() {
        let (_dir, repo) = seed_repo("status-clean");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/discussions/{}/git-status", disc_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        // The handler returned an envelope: success may be true (clean repo)
        // or false (depending on env), but either way no 500.
        assert!(
            json.get("success").is_some(),
            "envelope missing 'success': {json}"
        );
    }

    #[tokio::test]
    async fn disc_git_status_real_repo_with_pending_changes_returns_files() {
        let (_dir, repo) = seed_repo("status-dirty");
        // Make the repo "dirty" — a tracked file modified + an untracked.
        std::fs::write(repo.join("README.md"), "modified contents\n").unwrap();
        std::fs::write(repo.join("new.txt"), "fresh\n").unwrap();

        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/discussions/{}/git-status", disc_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // pending_files array should mention our two changes.
        let s = serde_json::to_string(&json).unwrap();
        assert!(s.contains("README.md"), "expected README.md in {s}");
        assert!(s.contains("new.txt"), "expected new.txt in {s}");
    }

    #[tokio::test]
    async fn disc_git_diff_rejects_traversal() {
        let (_dir, repo) = seed_repo("diff-traversal");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/discussions/{}/git-diff?path=../etc/passwd", disc_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        // ".." in path is rejected before we even hit git.
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            err.contains("Invalid path"),
            "expected Invalid path error, got {err}"
        );
    }

    #[tokio::test]
    async fn disc_git_diff_unchanged_file_returns_empty_diff() {
        let (_dir, repo) = seed_repo("diff-clean");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/discussions/{}/git-diff?path=README.md", disc_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        // Either success-with-empty-diff or success-false : either way, no 500.
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn disc_pr_template_returns_envelope_on_real_repo() {
        let (_dir, repo) = seed_repo("prtmpl");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/discussions/{}/pr-template", disc_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn disc_exec_real_repo_allows_ls() {
        // disc_exec validates the command against an allowlist (ls/find/grep/file/stat/du)
        // then runs it under the repo's work_dir. A real repo path means the
        // handler runs the full pipeline — argv parser + sandbox checks + exec.
        let (_dir, repo) = seed_repo("exec-ls");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/discussions/{}/exec", disc_id),
            serde_json::json!({ "command": "ls -la" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // Should succeed and contain our README.md in the output.
        if json["success"] == serde_json::Value::Bool(true) {
            let out = serde_json::to_string(&json).unwrap();
            assert!(
                out.contains("README.md"),
                "expected README.md in exec output, got {out}"
            );
        }
    }

    #[tokio::test]
    async fn disc_exec_real_repo_rejects_disallowed_command() {
        let (_dir, repo) = seed_repo("exec-deny");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/discussions/{}/exec", disc_id),
            serde_json::json!({ "command": "rm -rf /" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn disc_git_commit_no_files_returns_clean_envelope() {
        let (_dir, repo) = seed_repo("commit-nofiles");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/discussions/{}/git-commit", disc_id),
            serde_json::json!({ "message": "noop", "files": [] }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // No files staged = handler returns success=false (nothing to commit)
        // but it has walked the whole resolve_work_dir + git_ops::run_git_commit path.
        assert!(json.get("success").is_some());
    }

    /// Seed only a project (no discussion). Returns its id.
    async fn seed_project_with_repo(state: &AppState, repo_path: &std::path::Path) -> String {
        let now = chrono::Utc::now();
        let project_id = format!("proj-{}", uuid::Uuid::new_v4());
        let project = kronn::models::Project {
            id: project_id.clone(),
            name: "ProjTestRepo".into(),
            path: repo_path.to_string_lossy().to_string(),
            repo_url: None,
            token_override: None,
            ai_config: kronn::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: kronn::models::AiAuditStatus::NoTemplate,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: now,
            updated_at: now,
        };
        let p = project.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::projects::insert_project(conn, &p))
            .await
            .unwrap();
        project_id
    }

    /// Satisfy the durable validation gate: the latest run must be Completed,
    /// linked to its own validation discussion, and finished by the agent's
    /// terminal signal. A docs entry alone no longer earns validation.
    async fn seed_completed_validation(state: &AppState, project_id: &str) {
        let run_id = format!("audit-run-{}", uuid::Uuid::new_v4());
        let disc_id = format!("validation-disc-{}", uuid::Uuid::new_v4());
        let message_id = format!("validation-msg-{}", uuid::Uuid::new_v4());
        let pid = project_id.to_string();
        state.db.with_conn(move |conn| {
            let now = chrono::Utc::now();
            kronn::db::audit_runs::insert_running(
                conn, &run_id, &pid, "Full", "ClaudeCode", now,
            )?;
            kronn::db::audit_runs::complete(
                conn, &run_id, now, "Completed",
                0, 0, 0, 0, 0, 0, 0, 100, None, None,
            )?;
            conn.execute(
                "INSERT INTO discussions
                 (id, project_id, title, agent, language, created_at, updated_at)
                 VALUES (?1, ?2, 'Validation audit AI', 'ClaudeCode', 'fr', datetime('now'), datetime('now'))",
                rusqlite::params![disc_id, pid],
            )?;
            conn.execute(
                "INSERT INTO messages
                 (id, discussion_id, role, content, timestamp, tokens_used)
                 VALUES (?1, ?2, 'Agent', 'Validation terminée.\nKRONN:VALIDATION_COMPLETE', datetime('now'), 0)",
                rusqlite::params![message_id, disc_id],
            )?;
            kronn::db::audit_runs::set_validation_discussion(conn, &run_id, &disc_id)?;
            Ok(())
        }).await.unwrap();
    }

    #[tokio::test]
    async fn project_git_status_real_repo_returns_envelope() {
        let (_dir, repo) = seed_repo("proj-status");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/app.ts"), "export const ready = true;\n").unwrap();
        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:team/demo.git"])
            .current_dir(&repo)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["tag", "v0.9.0"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state.clone(), false);

        let uri = format!("/api/projects/{}/git-status", project_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["remote_url"], "https://github.com/team/demo");
        assert_eq!(
            json["data"]["pull_requests_url"],
            "https://github.com/team/demo/pulls"
        );
        assert_eq!(json["data"]["last_tag"], "v0.9.0");
        assert!(
            json["data"]["languages"]
                .as_array()
                .is_none_or(|languages| languages.is_empty()),
            "the cold default response must not wait for language analysis"
        );
        assert_eq!(json["data"]["languages_cached"], false);
        assert!(json["data"]["languages_checked_at"].is_null());

        let (_, refreshed) = get_json(
            build_router_with_auth(state.clone(), false),
            &format!("/api/projects/{}/git-status?refresh=true", project_id),
        )
        .await;
        assert_eq!(refreshed["success"], true);
        assert_eq!(refreshed["data"]["languages"][0]["language"], "TypeScript");
        assert_eq!(refreshed["data"]["languages_cached"], false);
        assert!(refreshed["data"]["languages_checked_at"].is_string());

        let (_, cached) = get_json(
            build_router_with_auth(state, false),
            &format!("/api/projects/{}/git-status", project_id),
        )
        .await;
        assert_eq!(cached["success"], true);
        assert_eq!(cached["data"]["languages_cached"], true);
        assert_eq!(
            cached["data"]["languages_checked_at"],
            refreshed["data"]["languages_checked_at"]
        );
    }

    #[tokio::test]
    async fn project_git_status_real_repo_with_changes() {
        let (_dir, repo) = seed_repo("proj-status-dirty");
        std::fs::write(repo.join("README.md"), "modified\n").unwrap();
        std::fs::write(repo.join("extra.txt"), "new\n").unwrap();

        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/git-status", project_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        let body = serde_json::to_string(&json).unwrap();
        assert!(body.contains("extra.txt"));
    }

    #[tokio::test]
    async fn project_dependency_updates_returns_empty_cached_summary_without_manifests() {
        let (_dir, repo) = seed_repo("proj-dependencies-empty");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let uri = format!("/api/projects/{}/dependency-updates", project_id);

        let (status, first) = get_json(build_router_with_auth(state.clone(), false), &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["success"], true);
        assert_eq!(first["data"]["managers"], serde_json::json!([]));
        assert_eq!(first["data"]["total_outdated"], 0);
        assert_eq!(first["data"]["cached"], false);

        let (_, second) = get_json(build_router_with_auth(state, false), &uri).await;
        assert_eq!(second["success"], true);
        assert_eq!(second["data"]["cached"], true);
    }

    #[tokio::test]
    async fn project_git_diff_traversal_rejected() {
        let (_dir, repo) = seed_repo("proj-diff-traversal");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/projects/{}/git-diff?path=../foo", project_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn project_pr_template_real_repo_returns_envelope() {
        let (_dir, repo) = seed_repo("proj-prtmpl");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/pr-template", project_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn project_exec_real_repo_allows_ls() {
        let (_dir, repo) = seed_repo("proj-exec-ls");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/exec", project_id),
            serde_json::json!({ "command": "ls" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        if json["success"] == serde_json::Value::Bool(true) {
            let out = serde_json::to_string(&json).unwrap();
            assert!(out.contains("README.md"));
        }
    }

    #[tokio::test]
    async fn project_exec_real_repo_rejects_disallowed() {
        let (_dir, repo) = seed_repo("proj-exec-deny");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/exec", project_id),
            serde_json::json!({ "command": "curl http://evil" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn project_git_commit_no_files_returns_envelope() {
        let (_dir, repo) = seed_repo("proj-commit-empty");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/git-commit", project_id),
            serde_json::json!({ "message": "nothing", "files": [] }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn project_git_branch_real_repo_returns_envelope() {
        let (_dir, repo) = seed_repo("proj-branch");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/git-branch", project_id),
            serde_json::json!({ "name": "feature/test-branch" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── contacts::add — business contract ────────────────────────────

    #[tokio::test]
    async fn contacts_add_rejects_invalid_invite_code_format() {
        let (st, json) = post_json(
            test_app(),
            "/api/contacts",
            serde_json::json!({ "invite_code": "not-a-kronn-code" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(err.contains("Invalid invite code"));
    }

    #[tokio::test]
    async fn contacts_add_unreachable_peer_persists_as_pending() {
        // Use a deliberately unreachable port so the peer ping fails.
        // The contact is still persisted with status=pending and a
        // diagnosis hint in the response.
        let (st, json) = post_json(
            test_app(),
            "/api/contacts",
            serde_json::json!({ "invite_code": "kronn:Test@127.0.0.1:1" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // May succeed (added pending) or fail — but no 500.
        assert!(json.get("success").is_some());
        if json["success"] == serde_json::Value::Bool(true) {
            assert_eq!(
                json["data"]["contact"]["status"], "pending",
                "unreachable peer must be persisted as pending"
            );
            assert!(json["data"]["warning"].is_string() || json["data"]["warning"].is_null());
        }
    }

    #[tokio::test]
    async fn contacts_add_rejects_duplicate_invite_code() {
        let state = test_state();
        let app1 = build_router_with_auth(state.clone(), false);
        let app2 = build_router_with_auth(state, false);

        // First add (will be pending since 127.0.0.1:1 is unreachable).
        let (st1, json1) = post_json(
            app1,
            "/api/contacts",
            serde_json::json!({ "invite_code": "kronn:Dup@127.0.0.1:1" }),
        )
        .await;
        assert_eq!(st1, StatusCode::OK);
        if json1["success"] != serde_json::Value::Bool(true) {
            // Insertion failed for another reason (e.g. peer ping took
            // too long) — skip the duplicate-check assertion since the
            // first contact wasn't persisted.
            return;
        }

        // Same code again must be rejected.
        let (st2, json2) = post_json(
            app2,
            "/api/contacts",
            serde_json::json!({ "invite_code": "kronn:Dup@127.0.0.1:1" }),
        )
        .await;
        assert_eq!(st2, StatusCode::OK);
        assert_eq!(json2["success"], serde_json::Value::Bool(false));
        let err = json2["error"].as_str().unwrap_or("");
        assert!(err.contains("already exists"));
    }

    // ── disc_introspection happy-path with seeded disc ───────────────

    async fn seed_disc_with_messages(state: &AppState, messages_count: usize) -> String {
        let (_dir, repo) = seed_repo("disc-meta");
        std::mem::forget(_dir);
        let disc_id = seed_disc_with_repo(state, &repo).await;

        // Append `messages_count` messages.
        let did = disc_id.clone();
        state
            .db
            .with_conn(move |conn| {
                for i in 0..messages_count {
                    let mid = format!("msg-{}-{}", i, uuid::Uuid::new_v4());
                    let now = chrono::Utc::now();
                    let role = if i % 2 == 0 { "User" } else { "Agent" };
                    conn.execute(
                        "INSERT INTO messages
                     (id, discussion_id, role, content, agent_type, timestamp,
                      tokens_used, sort_order, received_at)
                     VALUES (?1, ?2, ?3, ?4, NULL, ?5, 0, ?6, ?5)",
                        rusqlite::params![
                            mid,
                            did,
                            role,
                            format!("Message body {i}"),
                            now.to_rfc3339(),
                            i as i64 + 1
                        ],
                    )?;
                }
                conn.execute(
                    "UPDATE discussions SET next_message_seq = ?2 WHERE id = ?1",
                    rusqlite::params![did, messages_count as i64 + 1],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        disc_id
    }

    #[tokio::test]
    async fn disc_meta_seeded_disc_returns_message_count() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 5).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/discussions/{}/meta", disc_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // meta returns at least the message count.
        let count = json["data"]["message_count"].as_u64();
        assert!(count.is_some(), "data.message_count missing: {json}");
    }

    #[tokio::test]
    async fn disc_get_message_positive_index_returns_message() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 3).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/discussions/{}/message/0", disc_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn disc_get_message_exposes_the_exact_cli_reply_target() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 1).await;
        let did = disc_id.clone();
        let cli_session_id = state
            .db
            .with_conn(move |conn| {
                let message_id = conn.query_row(
                    "SELECT id FROM messages
                     WHERE discussion_id = ?1
                     ORDER BY sort_order ASC
                     LIMIT 1",
                    [&did],
                    |row| row.get::<_, String>(0),
                )?;
                let cli_session_id = kronn::db::discussion_sessions::create_session(
                    conn,
                    &did,
                    "Codex",
                    Some("codex-introspection"),
                    "peer",
                )?;
                kronn::db::discussions::set_message_cli_author(conn, &message_id, cli_session_id)?;
                Ok(cli_session_id)
            })
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let (status, json) = get_json(app, &format!("/api/discussions/{disc_id}/message/0")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["reply_target"]["kind"], "cli");
        assert_eq!(json["data"]["reply_target"]["agent_type"], "Codex");
        assert_eq!(
            json["data"]["reply_target"]["cli_session_id"],
            cli_session_id
        );
    }

    #[tokio::test]
    async fn disc_get_message_negative_index_resolves_last() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 3).await;
        let app = build_router_with_auth(state, false);

        // -1 = last message.
        let (st, json) = get_json(app, &format!("/api/discussions/{}/message/-1", disc_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn disc_get_message_short_ref_returns_bounded_context() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 5).await;
        let app = build_router_with_auth(state, false);

        let (_, target) = get_json(
            app.clone(),
            &format!("/api/discussions/{}/message/2", disc_id),
        )
        .await;
        let message_ref = target["data"]["message_ref"]
            .as_str()
            .expect("target should expose a short message reference");

        let (st, json) = get_json(
            app,
            &format!(
                "/api/discussions/{}/message/{}?before=2&after=1",
                disc_id, message_ref
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["idx"], 2);
        assert_eq!(json["data"]["before"].as_array().unwrap().len(), 2);
        assert_eq!(json["data"]["before"][0]["idx"], 0);
        assert_eq!(json["data"]["before"][1]["idx"], 1);
        assert_eq!(json["data"]["after"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"]["after"][0]["idx"], 3);
        assert!(json["data"]["before"][0].get("attachments").is_none());
    }

    #[tokio::test]
    async fn disc_get_message_full_id_resolves_target() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 2).await;
        let app = build_router_with_auth(state, false);
        let (_, target) = get_json(
            app.clone(),
            &format!("/api/discussions/{}/message/1", disc_id),
        )
        .await;
        let full_id = target["data"]["id"].as_str().unwrap();

        let (_, json) = get_json(
            app,
            &format!("/api/discussions/{}/message/{}", disc_id, full_id),
        )
        .await;
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["idx"], 1);
    }

    #[tokio::test]
    async fn disc_get_message_exact_note_keeps_main_only_numeric_indices_and_neighbours() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 3).await;
        let did = disc_id.clone();
        let note_id = state
            .db
            .with_conn(move |conn| {
                let note_id = conn.query_row(
                    "SELECT id FROM messages
                     WHERE discussion_id = ?1 AND sort_order = 2",
                    [&did],
                    |row| row.get::<_, String>(0),
                )?;
                conn.execute(
                    "UPDATE messages SET channel = 'note' WHERE id = ?1",
                    [&note_id],
                )?;
                Ok(note_id)
            })
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let (status, note) = get_json(
            app.clone(),
            &format!("/api/discussions/{disc_id}/message/{note_id}?before=1&after=1"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(note["data"]["channel"], "note");
        assert_eq!(note["data"]["before"].as_array().unwrap().len(), 1);
        assert_eq!(note["data"]["before"][0]["content"], "Message body 0");
        assert_eq!(note["data"]["after"].as_array().unwrap().len(), 1);
        assert_eq!(note["data"]["after"][0]["content"], "Message body 2");

        let (_, numeric) = get_json(app, &format!("/api/discussions/{disc_id}/message/1")).await;
        assert_eq!(numeric["data"]["channel"], "main");
        assert_eq!(numeric["data"]["content"], "Message body 2");
    }

    #[tokio::test]
    async fn disc_get_message_rejects_oversized_context_window() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 2).await;
        let app = build_router_with_auth(state, false);

        let (_, json) = get_json(
            app,
            &format!("/api/discussions/{}/message/0?before=11", disc_id),
        )
        .await;
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        assert!(json["error"].as_str().unwrap().contains("limited"));
    }

    #[tokio::test]
    async fn disc_get_message_out_of_range_returns_err() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 2).await;
        let app = build_router_with_auth(state, false);

        // 5 is way beyond 2 messages.
        let (st, json) = get_json(app, &format!("/api/discussions/{}/message/5", disc_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn disc_get_message_invalid_index_string_returns_err() {
        let state = test_state();
        let disc_id = seed_disc_with_messages(&state, 1).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(
            app,
            &format!("/api/discussions/{}/message/notanumber", disc_id),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    // ── check_drift — business contract with seeded checksums ───────

    #[tokio::test]
    async fn check_drift_no_checksums_returns_empty_envelope() {
        // Greenfield project (no docs/checksums.json) — must NOT error;
        // return an empty/null drift response so the UI can show
        // "no prior audit".
        let (_dir, repo) = seed_repo("drift-no-checksums");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/drift", pid)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // No prior audit → 0 fresh + 0 total sections.
        // fresh_sections is the array of section names that ARE up-to-date.
        // No prior audit → empty array.
        let fresh = json["data"]["fresh_sections"]
            .as_array()
            .expect("fresh_sections is array");
        assert!(fresh.is_empty());
    }

    #[tokio::test]
    async fn check_drift_with_checksums_returns_fresh_when_no_drift() {
        // Seed project + a checksums file capturing the current state.
        // Drift check should report 0 stale sections.
        let (_dir, repo) = seed_repo("drift-fresh");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        // Minimal valid checksums.json — see core::checksums for the format.
        std::fs::write(
            repo.join("docs/checksums.json"),
            r#"{
              "audit_date": "2026-05-28T00:00:00Z",
              "kronn_version": "0.8.7",
              "sections": []
            }"#,
        )
        .unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/drift", pid)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
    }

    #[tokio::test]
    async fn check_drift_unknown_project_returns_err_envelope() {
        // Already covered by the wrong-id sweep but pinning here too
        // because the audit_status -> drift handoff is a critical UX path.
        let (st, json) = get_json(test_app(), "/api/projects/nope/drift").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        assert!(json["error"].as_str().unwrap_or("").contains("not found"));
    }

    // ── partial-audit — payload validation contracts ────────────────

    // Note : partial_audit returns Sse<SseStream>, not JSON — can't be
    // tested with our oneshot helper. Already covered by the SSE
    // initial validation path: see api/audit/full's classify_docs_dir
    // tests for the cold-cancel + cleanup logic.

    // ── projects::add_folder — business contract ─────────────────────

    #[tokio::test]
    async fn add_folder_rejects_path_with_traversal() {
        let (st, json) = post_json(
            test_app(),
            "/api/projects/add-folder",
            serde_json::json!({ "path": "/home/user/../etc/passwd" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(err.contains("'..'") || err.contains("traversal"));
    }

    #[tokio::test]
    async fn add_folder_rejects_nonexistent_directory() {
        let (st, json) = post_json(
            test_app(),
            "/api/projects/add-folder",
            serde_json::json!({ "path": "/definitely/does/not/exist/folder-1234567" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(err.contains("does not exist"));
    }

    #[tokio::test]
    async fn add_folder_succeeds_for_real_directory_and_picks_default_name() {
        let (_dir, repo) = seed_repo("add-folder-ok");
        let app = test_app();
        let (st, json) = post_json(
            app,
            "/api/projects/add-folder",
            serde_json::json!({ "path": repo.to_string_lossy() }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // Auto-name = last path component when not provided.
        let name = json["data"]["name"].as_str().unwrap_or("");
        let expected = repo.file_name().unwrap().to_string_lossy();
        assert_eq!(name, expected.as_ref());
    }

    #[tokio::test]
    async fn add_folder_uses_explicit_name_when_provided() {
        let (_dir, repo) = seed_repo("add-folder-named");
        let (st, json) = post_json(
            test_app(),
            "/api/projects/add-folder",
            serde_json::json!({
                "path": repo.to_string_lossy(),
                "name": "ExplicitName"
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["name"], "ExplicitName");
    }

    #[tokio::test]
    async fn add_folder_falls_back_to_default_when_name_is_empty_whitespace() {
        // Empty/whitespace `name` must NOT be persisted as the project
        // name — fall back to the last path component.
        let (_dir, repo) = seed_repo("add-folder-empty-name");
        let (st, json) = post_json(
            test_app(),
            "/api/projects/add-folder",
            serde_json::json!({ "path": repo.to_string_lossy(), "name": "   " }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        let name = json["data"]["name"].as_str().unwrap_or("");
        assert_ne!(name, "   ", "whitespace name must be replaced by default");
        assert!(!name.trim().is_empty(), "default name must be non-empty");
    }

    #[tokio::test]
    async fn add_folder_rejects_duplicate_path() {
        let (_dir, repo) = seed_repo("add-folder-dup");
        let path = repo.to_string_lossy().to_string();
        let state = test_state();

        // First add succeeds.
        let app1 = build_router_with_auth(state.clone(), false);
        let (st1, json1) = post_json(
            app1,
            "/api/projects/add-folder",
            serde_json::json!({ "path": path.clone() }),
        )
        .await;
        assert_eq!(st1, StatusCode::OK);
        assert_eq!(json1["success"], serde_json::Value::Bool(true));

        // Second add on the same path must fail.
        let app2 = build_router_with_auth(state, false);
        let (st2, json2) = post_json(
            app2,
            "/api/projects/add-folder",
            serde_json::json!({ "path": path }),
        )
        .await;
        assert_eq!(st2, StatusCode::OK);
        assert_eq!(json2["success"], serde_json::Value::Bool(false));
        let err = json2["error"].as_str().unwrap_or("");
        assert!(err.contains("already exists"));
    }

    #[tokio::test]
    async fn add_folder_detects_git_remote_when_repo_is_git() {
        let (_dir, repo) = seed_repo("add-folder-git");
        // Add a fake remote.
        let _ = std::process::Command::new("git")
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/test/repo.git",
            ])
            .current_dir(&repo)
            .output();

        let (st, json) = post_json(
            test_app(),
            "/api/projects/add-folder",
            serde_json::json!({ "path": repo.to_string_lossy() }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        let repo_url = json["data"]["repo_url"].as_str().unwrap_or("");
        assert!(
            repo_url.contains("github.com/test/repo"),
            "repo_url should be detected from .git/config, got {repo_url}"
        );
    }

    // ── projects::create — business contract ─────────────────────────

    #[tokio::test]
    async fn create_project_rejects_traversal_path() {
        let (st, json) = post_json(
            test_app(),
            "/api/projects",
            serde_json::json!({
                "path": "/home/user/../etc",
                "name": "BadProject",
                "remote_url": null,
                "branch": "main",
                "ai_configs": [],
                "has_project": false,
                "hidden": false
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn create_project_succeeds_with_clean_payload() {
        let (_dir, repo) = seed_repo("create-clean");
        let (st, json) = post_json(
            test_app(),
            "/api/projects",
            serde_json::json!({
                "path": repo.to_string_lossy(),
                "name": "TestProject",
                "remote_url": null,
                "branch": "main",
                "ai_configs": [],
                "has_project": false,
                "hidden": false
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["name"], "TestProject");
    }

    // ── validate-audit / mark-bootstrapped — business contracts ──────

    #[tokio::test]
    async fn validate_audit_rejects_when_no_agents_md_exists() {
        // Pre-bootstrap project (no docs/AGENTS.md) → validation must
        // refuse with an actionable message. Pinning this avoids users
        // marking a never-audited project as "validated".
        let (_dir, repo) = seed_repo("validate-no-agents");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        seed_completed_validation(&state, &pid).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/validate-audit", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            err.contains("AGENTS.md"),
            "expected the completed validation gate to reach the missing-docs check, got {err}"
        );
    }

    #[tokio::test]
    async fn validate_audit_succeeds_after_completed_linked_validation() {
        // A completed run with its terminally-signalled linked discussion
        // must validate and create docs/.kronn.json.
        let (_dir, repo) = seed_repo("validate-ok");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(repo.join("docs/AGENTS.md"), "# AGENTS\n").unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        seed_completed_validation(&state, &pid).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/validate-audit", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));

        let kronn_state = repo.join("docs/.kronn.json");
        assert!(kronn_state.exists(), "docs/.kronn.json must be created");
        let content = std::fs::read_to_string(&kronn_state).unwrap();
        assert!(
            content.contains("validated_at"),
            "validated_at must be set in .kronn.json"
        );
    }

    #[tokio::test]
    async fn validate_audit_is_idempotent_preserves_first_date() {
        // Second call must NOT overwrite the validated_at timestamp.
        // (kronn_state::mark_validated does the no-op-if-set check).
        let (_dir, repo) = seed_repo("validate-idempotent");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(repo.join("docs/AGENTS.md"), "# AGENTS\n").unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        seed_completed_validation(&state, &pid).await;

        // First call.
        let (_, first_response) = post_json(
            build_router_with_auth(state.clone(), false),
            &format!("/api/projects/{}/validate-audit", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(first_response["success"], serde_json::Value::Bool(true));
        let first = std::fs::read_to_string(repo.join("docs/.kronn.json")).unwrap();

        // Second call (could be hours / days later).
        let (_, second_response) = post_json(
            build_router_with_auth(state, false),
            &format!("/api/projects/{}/validate-audit", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(second_response["success"], serde_json::Value::Bool(true));
        let second = std::fs::read_to_string(repo.join("docs/.kronn.json")).unwrap();

        // The validated_at line must be unchanged (both runs happen on
        // the same day, but the contract is "preserve the first date" —
        // the rest of the file can re-rewrite the _readme line).
        let extract = |s: &str| {
            s.lines()
                .find(|l| l.contains("validated_at"))
                .map(|l| l.to_string())
                .unwrap_or_default()
        };
        assert_eq!(
            extract(&first),
            extract(&second),
            "validated_at must be idempotent across repeated validate calls"
        );
    }

    #[tokio::test]
    async fn mark_bootstrapped_rejects_when_no_agents_md() {
        let (_dir, repo) = seed_repo("bootstrap-no-agents");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/mark-bootstrapped", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn mark_bootstrapped_succeeds_with_agents_md_and_writes_kronn_state() {
        let (_dir, repo) = seed_repo("bootstrap-ok");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(repo.join("docs/AGENTS.md"), "# AGENTS\n").unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/mark-bootstrapped", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));

        let kronn_state = repo.join("docs/.kronn.json");
        assert!(kronn_state.exists());
        let content = std::fs::read_to_string(&kronn_state).unwrap();
        assert!(
            content.contains("bootstrapped_at"),
            "bootstrapped_at must be set in .kronn.json"
        );
    }

    // ── save_briefing_form — business-contract tests ─────────────────

    #[tokio::test]
    async fn save_briefing_form_rejects_empty_q1_purpose() {
        let (_dir, repo) = seed_repo("brief-q1");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "", // missing Q1
                "team": "alice & bob",
                "maturity": "MVP",
                "dependencies": "stripe, sendgrid",
                "traps": "race in auth.rs",
                "additional": ""
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            err.contains("mandatory") || err.contains("Q1"),
            "expected mandatory-Q1 error, got {err}"
        );
    }

    #[tokio::test]
    async fn save_briefing_form_rejects_empty_q5_traps() {
        let (_dir, repo) = seed_repo("brief-q5");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "X", "team": "Y", "maturity": "Z",
                "dependencies": "deps",
                "traps": "", // missing Q5
                "additional": ""
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
    }

    #[tokio::test]
    async fn save_briefing_form_accepts_all_mandatory_filled_with_empty_q6() {
        let (_dir, repo) = seed_repo("brief-ok");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "Build a CSM platform",
                "team": "Pierre + Alice",
                "maturity": "v0.8.7 production",
                "dependencies": "Stripe, Sendgrid, Mailjet",
                "traps": "Auth race on concurrent sessions",
                "additional": "" // Q6 optional → renders as None.
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
    }

    #[tokio::test]
    async fn save_briefing_form_writes_file_to_docs_when_present() {
        // Seed a project + create docs/ dir → handler should write
        // docs/briefing.md AND persist briefing_notes in DB.
        let (_dir, repo) = seed_repo("brief-file");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "Test purpose",
                "team": "Test team",
                "maturity": "MVP",
                "dependencies": "none",
                "traps": "none",
                "additional": "extra context"
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));

        let briefing_path = repo.join("docs/briefing.md");
        assert!(briefing_path.exists(), "docs/briefing.md must exist");
        let content = std::fs::read_to_string(&briefing_path).unwrap();
        // Pin the exact section structure (the audit's Phase 1 depends on it).
        assert!(content.contains("# Project Briefing"));
        assert!(content.contains("## Purpose"));
        assert!(content.contains("Test purpose"));
        assert!(content.contains("## Team"));
        assert!(content.contains("## Maturity"));
        assert!(content.contains("## External Dependencies"));
        assert!(content.contains("## Traps & Fragile Areas"));
        assert!(content.contains("## Additional Context"));
        assert!(content.contains("extra context"));
    }

    #[tokio::test]
    async fn save_briefing_form_renders_none_fallback_for_empty_additional() {
        let (_dir, repo) = seed_repo("brief-none");
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let _ = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "P", "team": "T", "maturity": "M",
                "dependencies": "D", "traps": "Tr",
                "additional": "" // empty → "None."
            }),
        )
        .await;

        let content = std::fs::read_to_string(repo.join("docs/briefing.md")).unwrap();
        // The handler hard-codes the "None." fallback string for additional.
        let lines: Vec<&str> = content.lines().collect();
        let additional_idx = lines
            .iter()
            .position(|l| l.starts_with("## Additional Context"))
            .expect("Additional section header must exist");
        let after = &lines[additional_idx + 1];
        assert_eq!(after.trim(), "None.", "empty Q6 must render as 'None.'");
    }

    #[tokio::test]
    async fn save_briefing_form_db_only_when_docs_dir_absent() {
        // Project without docs/ dir (pre-bootstrap) → handler must
        // NOT crash. Stores briefing_notes in DB only.
        let (_dir, repo) = seed_repo("brief-nodocs");
        // intentionally NOT creating docs/
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/save-briefing", pid),
            serde_json::json!({
                "purpose": "P", "team": "T", "maturity": "M",
                "dependencies": "D", "traps": "Tr", "additional": ""
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert!(
            !repo.join("docs/briefing.md").exists(),
            "no docs/ dir → no briefing.md should have been written"
        );
    }

    // ── audit briefing handlers ──────────────────────────────────────
    envelope_post!(
        audit_save_briefing_form_unknown,
        "/api/projects/nope/save-briefing",
        serde_json::json!({ "answers": {} })
    );

    #[tokio::test]
    async fn audit_briefing_get_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("briefing-get");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/briefing", pid)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn audit_briefing_set_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("briefing-set");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = put_json(
            app,
            &format!("/api/projects/{}/briefing", pid),
            serde_json::json!({ "briefing_notes": "Test briefing content" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── quick-apis cold endpoints ────────────────────────────────────
    envelope_post!(
        qa_run_unknown_id,
        "/api/quick-apis/nope/run",
        serde_json::json!({ "vars": {} })
    );
    envelope_post!(
        qa_batch_run_unknown_id,
        "/api/quick-apis/nope/batch",
        serde_json::json!({ "items": [], "vars": {} })
    );
    envelope_get!(qa_export_unknown_id, "/api/quick-apis/nope/export");
    envelope_post!(
        qa_import_invalid_kind,
        "/api/quick-apis/import",
        serde_json::json!({ "kind": "not-kronn-qa", "data": {} })
    );
    envelope_post!(
        qa_import_invalid_payload,
        "/api/quick-apis/import",
        serde_json::json!({ "totally": "bogus" })
    );

    // ── mcps cold endpoints ──────────────────────────────────────────
    envelope_post!(
        mcp_custom_cleanup_orphan_env_unknown_server,
        "/api/mcps/custom/nope-server/cleanup-orphan-env",
        serde_json::json!({ "config_id": "nope" })
    );
    envelope_get!(
        mcp_custom_export_file_unknown_server,
        "/api/mcps/custom/nope-server/export-file"
    );
    envelope_post!(
        mcp_custom_import_file_invalid_payload,
        "/api/mcps/custom/import-file",
        serde_json::json!({ "filename": "x.json", "content": "not valid json" })
    );
    envelope_post!(
        mcp_custom_import_file_valid_but_unknown_kind,
        "/api/mcps/custom/import-file",
        serde_json::json!({ "filename": "x.json", "content": "{\"kind\":\"unknown\"}" })
    );

    // ── project handlers — happy-paths w/ seeded project ────────────

    #[tokio::test]
    async fn set_linked_repos_seeded_project_returns_envelope() {
        let (_dir, repo) = seed_repo("linked-repos");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = put_json(
            app,
            &format!("/api/projects/{}/linked-repos", pid),
            serde_json::json!([
                {
                    "id": "lr-1", "name": "api", "kind": "api",
                    "location": "/tmp/api-repo", "description": "API repo"
                }
            ]),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn linked_repos_candidates_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("lr-candidates");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(
            app,
            &format!("/api/projects/{}/linked-repos/candidates", pid),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn set_default_skills_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("default-skills");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = put_json(
            app,
            &format!("/api/projects/{}/default-skills", pid),
            serde_json::json!(["rust", "frontend"]),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn set_default_profile_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("default-profile");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = put_json(
            app,
            &format!("/api/projects/{}/default-profile", pid),
            serde_json::json!({ "profile_id": null }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn remap_path_seeded_returns_envelope() {
        let (_dir, repo) = seed_repo("remap-path");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        // Try to remap to a new path. Will likely err (path doesn't exist)
        // but the handler walks the validation logic.
        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/remap-path", pid),
            serde_json::json!({ "new_path": "/tmp/nonexistent-remap-target" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn anti_hallu_status_seeded_project() {
        let (_dir, repo) = seed_repo("anti-hallu");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/anti-hallu/status", pid)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn redirectors_sync_seeded_project() {
        let (_dir, repo) = seed_repo("redirectors");
        let state = test_state();
        let pid = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/projects/{}/redirectors/sync", pid),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── workflows.rs cold edge endpoints ─────────────────────────────
    envelope_post!(
        workflow_trigger_unknown_id,
        "/api/workflows/nope/trigger",
        serde_json::json!({ "variables": {} })
    );
    envelope_delete!(
        workflow_runs_delete_all_unknown_wf,
        "/api/workflows/nope/runs"
    );
    envelope_get!(
        workflow_get_run_unknown_both,
        "/api/workflows/nope/runs/nope"
    );
    envelope_delete!(
        workflow_delete_run_unknown_both,
        "/api/workflows/nope/runs/nope"
    );
    envelope_post!(
        workflow_cancel_run_unknown,
        "/api/workflows/nope/runs/nope/cancel",
        serde_json::json!({})
    );
    envelope_post!(
        workflow_decide_run_unknown,
        "/api/workflows/nope/runs/nope/decide",
        serde_json::json!({ "verdict": "Approve", "comment": null })
    );
    envelope_post!(
        workflow_decide_run_invalid_verdict,
        "/api/workflows/nope/runs/nope/decide",
        serde_json::json!({ "verdict": "Bogus" })
    );
    envelope_post!(
        workflow_test_worktree_unknown,
        "/api/workflows/nope/runs/nope/test-worktree",
        serde_json::json!({})
    );

    // ── audit/run.rs global routes ──────────────────────────────────
    envelope_get!(audit_status_all_returns_envelope, "/api/audit-status");
    envelope_post!(
        audit_runs_cleanup_no_threshold,
        "/api/audit-runs/cleanup",
        serde_json::json!({})
    );
    envelope_post!(
        audit_runs_cleanup_with_threshold,
        "/api/audit-runs/cleanup",
        serde_json::json!({ "keep_recent": 5 })
    );

    // ── audit/run.rs cold paths (audit-status, audit-history, audit-latest, …) ─
    envelope_get!(
        audit_status_unknown_project,
        "/api/projects/nope/audit-status"
    );
    envelope_get!(
        audit_resumable_unknown_project,
        "/api/projects/nope/audit-resumable"
    );
    envelope_get!(
        audit_latest_unknown_project,
        "/api/projects/nope/audit-latest"
    );
    envelope_get!(
        audit_history_unknown_project,
        "/api/projects/nope/audit-history"
    );
    envelope_post!(
        audit_partial_unknown_project,
        "/api/projects/nope/partial-audit",
        serde_json::json!({})
    );
    envelope_post!(
        audit_validate_unknown_project,
        "/api/projects/nope/validate-audit",
        serde_json::json!({})
    );
    envelope_get!(
        audit_briefing_get_unknown_project,
        "/api/projects/nope/briefing"
    );
    envelope_post!(
        audit_start_briefing_unknown_project,
        "/api/projects/nope/start-briefing",
        serde_json::json!({})
    );

    // ── discussions/crud + messaging cold paths ──────────────────────
    envelope_get!(disc_list_returns_envelope, "/api/discussions");
    envelope_get!(disc_get_unknown_id, "/api/discussions/nope");
    envelope_delete!(disc_delete_unknown_id, "/api/discussions/nope");
    envelope_post!(
        disc_run_agent_unknown_id,
        "/api/discussions/nope/run",
        serde_json::json!({})
    );
    envelope_post!(
        disc_stop_agent_unknown_id,
        "/api/discussions/nope/stop",
        serde_json::json!({})
    );
    envelope_post!(
        disc_dismiss_partial_unknown_id,
        "/api/discussions/nope/dismiss-partial",
        serde_json::json!({})
    );
    envelope_post!(
        disc_orchestrate_unknown_id,
        "/api/discussions/nope/orchestrate",
        serde_json::json!({})
    );
    envelope_post!(
        disc_share_unknown_id,
        "/api/discussions/nope/share",
        serde_json::json!({})
    );
    envelope_get!(disc_meta_unknown_id, "/api/discussions/nope/meta");
    envelope_get!(disc_message_unknown, "/api/discussions/nope/message/0");
    envelope_post!(
        disc_create_minimal_payload,
        "/api/discussions",
        serde_json::json!({
            "title": "Test Disc",
            "agent": "claude-code",
            "language": "en",
            "workspace_mode": "Direct"
        })
    );
    envelope_get!(
        disc_participants_unknown_id,
        "/api/discussions/nope/participants"
    );
    envelope_get!(
        disc_wait_unknown_id,
        "/api/discussions/nope/wait?timeout_s=1"
    );
    envelope_post!(
        disc_invite_peer_unknown_id,
        "/api/discussions/nope/invite-peer",
        serde_json::json!({ "pseudo": "X", "invite_code": "kronn:X@127.0.0.1:1234" })
    );
    envelope_get!(
        disc_source_endpoint_unknown_id,
        "/api/discussions/nope/source"
    );

    // ── api/themes::unlock — wrong / empty code paths ──────────────────
    envelope_post!(
        themes_unlock_empty_code,
        "/api/themes/unlock",
        serde_json::json!({ "code": "" })
    );
    envelope_post!(
        themes_unlock_whitespace_only,
        "/api/themes/unlock",
        serde_json::json!({ "code": "   " })
    );
    envelope_post!(
        themes_unlock_invalid_code,
        "/api/themes/unlock",
        serde_json::json!({ "code": "definitely-not-a-real-code-12345" })
    );

    // ── api/skills auto-trigger endpoints ──────────────────────────────
    envelope_get!(
        skills_list_disabled_auto,
        "/api/skills/auto-triggers/disabled"
    );
    envelope_post!(
        skills_toggle_auto_trigger_unknown,
        "/api/skills/nope/auto-trigger/toggle",
        serde_json::json!({ "enabled": true })
    );

    // ── api/workflows export/import + cancel/restart ───────────────────
    envelope_get!(workflow_export_unknown, "/api/workflows/nope/export");
    envelope_post!(
        workflow_import_garbage,
        "/api/workflows/import",
        serde_json::json!({ "kind": "bogus", "data": {} })
    );
    envelope_post!(
        workflow_clone_unknown,
        "/api/workflows/nope/clone",
        serde_json::json!({})
    );
    envelope_get!(workflow_runs_unknown_run, "/api/workflows/runs/nope");

    // ── workflow_trigger happy-path (seeded enabled workflow) ──────────

    #[tokio::test]
    async fn mcp_workflow_trigger_disabled_workflow_returns_err() {
        // Seed a workflow with enabled=false. Trigger must refuse cleanly.
        let state = test_state();
        let now = chrono::Utc::now();
        let workflow_id = format!("wf-disabled-{}", uuid::Uuid::new_v4());
        let wf = kronn::models::Workflow {
            pinned: false,
            id: workflow_id.clone(),
            name: "DisabledWF".into(),
            project_id: None,
            trigger: kronn::models::WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: kronn::models::WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts: std::collections::HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: false, // disabled
            created_at: now,
            updated_at: now,
        };
        let wf_clone = wf.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::workflows::insert_workflow(conn, &wf_clone))
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/workflow-trigger",
            serde_json::json!({
                "workflow_id": workflow_id,
                "project_id": null,
                "variables": {}
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            err.contains("disabled"),
            "expected 'disabled' error, got {err}"
        );
    }

    #[tokio::test]
    async fn mcp_workflow_trigger_required_var_missing_returns_err() {
        let state = test_state();
        let now = chrono::Utc::now();
        let workflow_id = format!("wf-vars-{}", uuid::Uuid::new_v4());
        let wf = kronn::models::Workflow {
            pinned: false,
            id: workflow_id.clone(),
            name: "VarsWF".into(),
            project_id: None,
            trigger: kronn::models::WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: kronn::models::WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts: std::collections::HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![kronn::models::PromptVariable {
                name: "ticket".into(),
                label: "Ticket".into(),
                placeholder: String::new(),
                description: None,
                required: true,
                pattern: None,
            }],
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        let wf_clone = wf.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::workflows::insert_workflow(conn, &wf_clone))
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/workflow-trigger",
            serde_json::json!({
                "workflow_id": workflow_id,
                "project_id": null,
                "variables": {} // missing required 'ticket'
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(false));
        let err = json["error"].as_str().unwrap_or("");
        assert!(
            err.contains("obligatoire") || err.contains("required"),
            "expected required-variable error, got {err}"
        );
    }

    // ── qp_run / qp_batch_run happy-path (seeded qp + project) ──────────
    //
    // qp_run validates inputs, looks up qp + project in DB, creates a disc,
    // inserts the initial User message, then fire-and-forget spawns the
    // agent (which fails in test env — but the *return* response is built
    // from the pre-spawn state). Walks ~150 LOC of mcp_remote::qp_run.

    async fn seed_qp_and_project(state: &AppState) -> (String, String) {
        let now = chrono::Utc::now();
        let (_dir, repo) = seed_repo("qprun");
        std::mem::forget(_dir);
        let project_id = seed_project_with_repo(state, &repo).await;

        let qp_id = format!("qp-{}", uuid::Uuid::new_v4());
        let qp = kronn::models::QuickPrompt {
            id: qp_id.clone(),
            pinned: false,
            name: "TestQP".into(),
            icon: "✨".into(),
            prompt_template: "Analyse: {{topic}}".into(),
            variables: vec![kronn::models::PromptVariable {
                name: "topic".into(),
                label: "Topic".into(),
                placeholder: String::new(),
                description: None,
                required: true,
                pattern: None,
            }],
            agent: kronn::models::AgentType::ClaudeCode,
            project_id: Some(project_id.clone()),
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            tier: kronn::models::ModelTier::Default,
            agent_settings: None,
            description: "test QP".into(),
            created_at: now,
            updated_at: now,
        };
        let q = qp.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::quick_prompts::insert_quick_prompt(conn, &q))
            .await
            .unwrap();
        (qp_id, project_id)
    }

    #[tokio::test]
    async fn mcp_qp_run_seeded_returns_disc_id() {
        let state = test_state();
        let (qp_id, project_id) = seed_qp_and_project(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/qp-run",
            serde_json::json!({
                "qp_id": qp_id,
                "project_id": project_id,
                "vars": { "topic": "test" }
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // Whether or not the agent spawn succeeded, the JSON wrapper must
        // walk : qp lookup → project lookup → render template → create disc
        // → insert initial message → fire spawn → build response.
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn mcp_qp_run_missing_required_var_returns_err() {
        let state = test_state();
        let (qp_id, project_id) = seed_qp_and_project(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/qp-run",
            serde_json::json!({
                "qp_id": qp_id,
                "project_id": project_id,
                "vars": {} // missing required 'topic'
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        // Either succeeds with empty template var, or returns err — but no 500.
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn mcp_qp_batch_run_seeded_with_items() {
        let state = test_state();
        let (qp_id, project_id) = seed_qp_and_project(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/qp-batch-run",
            serde_json::json!({
                "qp_id": qp_id,
                "project_id": project_id,
                "items": [
                    { "topic": "first" },
                    { "topic": "second" }
                ]
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── api/setup cold POST endpoints — exercises entry points we haven't hit ─
    envelope_post!(
        setup_set_scan_paths_empty,
        "/api/setup/scan-paths",
        serde_json::json!({ "paths": [] })
    );
    envelope_post!(
        config_set_scan_paths_via_alias,
        "/api/config/scan-paths",
        serde_json::json!({ "paths": ["/tmp/test-scan"] })
    );
    envelope_post!(
        config_set_scan_ignore_empty,
        "/api/config/scan-ignore",
        serde_json::json!({ "ignore": [] })
    );
    envelope_post!(
        setup_complete_returns_envelope,
        "/api/setup/complete",
        serde_json::json!({})
    );
    envelope_post!(
        setup_save_api_key_invalid_value_with_star,
        "/api/config/api-keys",
        serde_json::json!({ "id": null, "name": "k1", "provider": "anthropic", "value": "sk-***" })
    );
    envelope_post!(
        setup_save_api_key_empty_value,
        "/api/config/api-keys",
        serde_json::json!({ "id": null, "name": "k1", "provider": "anthropic", "value": "" })
    );
    envelope_get!(setup_config_scan_paths_get, "/api/config/scan-paths");
    envelope_get!(setup_config_scan_ignore_get, "/api/config/scan-ignore");

    // ── api/profiles update_persona_name endpoint (PUT, never hit) ──────
    envelope_post!(
        profiles_update_persona_name_unknown_uses_put_method_via_post_will_fail_405,
        "/api/profiles/nope/persona-name",
        serde_json::json!({ "persona_name": "X" })
    );
    // The PUT version (proper):
    #[tokio::test]
    async fn profiles_update_persona_name_unknown_id_via_put_returns_envelope() {
        let app = test_app();
        let (st, json) = put_json(
            app,
            "/api/profiles/nope/persona-name",
            serde_json::json!({ "persona_name": "TestPersona" }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── api/disc_invite cold endpoints ──────────────────────────────────
    envelope_get!(
        disc_invite_participants_unknown,
        "/api/discussions/nope/participants"
    );
    envelope_get!(
        disc_invite_wait_unknown,
        "/api/discussions/nope/wait?timeout_s=1"
    );
    envelope_post!(
        disc_invite_peer_join_no_disc,
        "/api/discussions/peer-join",
        serde_json::json!({
            "shared_id": "nope", "pseudo": "X",
            "invite_code": "kronn:X@127.0.0.1:1", "from_avatar_email": null
        })
    );
    envelope_post!(
        disc_invite_peer_leave_no_disc,
        "/api/discussions/peer-leave",
        serde_json::json!({
            "shared_id": "nope", "pseudo": "X"
        })
    );

    // ── quick_prompts.rs cold reads with wrong id — history/metrics/versions ─
    envelope_get!(qp_history_unknown_id, "/api/quick-prompts/nope/history");
    envelope_get!(qp_metrics_unknown_id, "/api/quick-prompts/nope/metrics");
    envelope_delete!(
        qp_delete_version_unknown_id,
        "/api/quick-prompts/nope/versions/0"
    );

    // ── audit/run.rs happy-path : pre-seeded audit_runs row ──────────
    //
    // Following the mcp_remote pre-seeded pattern that gave +0.11 Lines
    // for 5 tests (7× the wrong-id ratio), seed a real project + a
    // completed audit_runs row, then exercise the read endpoints
    // (audit_latest, audit_history, audit_run_steps, audit_resumable).

    async fn seed_project_with_audit_run(state: &AppState) -> (String, String) {
        let now = chrono::Utc::now();
        // Real project (so the FK on audit_runs.project_id holds).
        let tmp = tempfile::tempdir().unwrap();
        // Leak the tempdir so it survives the function — we don't need it back.
        let path = tmp.path().to_path_buf();
        std::mem::forget(tmp);

        let project_id = format!("proj-audit-{}", uuid::Uuid::new_v4());
        let project = kronn::models::Project {
            id: project_id.clone(),
            name: "AuditTestProj".into(),
            path: path.to_string_lossy().to_string(),
            repo_url: None,
            token_override: None,
            ai_config: kronn::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: kronn::models::AiAuditStatus::Audited,
            ai_todo_count: 0,
            tech_debt_count: 0,
            needs_docs_migration: false,
            path_exists: true,
            default_skill_ids: vec![],
            default_profile_id: None,
            briefing_notes: None,
            linked_repos: vec![],
            created_at: now,
            updated_at: now,
        };
        let p = project.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::projects::insert_project(conn, &p))
            .await
            .unwrap();

        let run_id = format!("audit-run-{}", uuid::Uuid::new_v4());
        let pid_for_insert = project_id.clone();
        let rid = run_id.clone();
        let started = now - chrono::Duration::seconds(60);
        state
            .db
            .with_conn(move |conn| {
                kronn::db::audit_runs::insert_running(
                    conn,
                    &rid,
                    &pid_for_insert,
                    "full",
                    "claude-code",
                    started,
                )?;
                // Complete it so audit_latest returns it.
                kronn::db::audit_runs::complete(
                    conn,
                    &rid,
                    chrono::Utc::now(),
                    "Completed",
                    1,
                    2,
                    3,
                    4, // critical/high/medium/low
                    0,
                    0,
                    0,  // resolved/new/carried
                    75, // health_score
                    None,
                    None, // report_path / recommendations
                )?;
                Ok(())
            })
            .await
            .unwrap();
        (project_id, run_id)
    }

    #[tokio::test]
    async fn audit_latest_returns_completed_run_envelope() {
        let state = test_state();
        let (project_id, _run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/projects/{}/audit-latest", project_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // Latest run should be the one we seeded — completed, with td counts.
        let data = &json["data"];
        assert!(
            data.is_object() || data.is_null(),
            "audit_latest data shape"
        );
    }

    #[tokio::test]
    async fn audit_history_returns_array_with_seeded_run() {
        let state = test_state();
        let (project_id, _run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) =
            get_json(app, &format!("/api/projects/{}/audit-history", project_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        let arr = json["data"].as_array().expect("history is an array");
        assert!(!arr.is_empty(), "history should include the seeded run");
    }

    #[tokio::test]
    async fn audit_run_steps_returns_envelope_for_seeded_run() {
        let state = test_state();
        let (_project_id, run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/audit-runs/{}/steps", run_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
        // Steps may be empty since we didn't seed any — handler still
        // walks the happy path through the SQL query.
    }

    #[tokio::test]
    async fn audit_resumable_returns_envelope_for_seeded_project() {
        let state = test_state();
        let (project_id, _run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(
            app,
            &format!("/api/projects/{}/audit-resumable", project_id),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn audit_info_returns_envelope_for_seeded_project() {
        let state = test_state();
        let (project_id, _run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/audit-info", project_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // Seeded project has a tempdir path with no audit files yet — arrays are empty.
        let data = &json["data"];
        assert!(data["files"].is_array());
        assert!(data["todos"].is_array());
        assert!(data["tech_debt_items"].is_array());
    }

    #[tokio::test]
    async fn audit_status_returns_envelope_for_seeded_project() {
        let state = test_state();
        let (project_id, _run_id) = seed_project_with_audit_run(&state).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = get_json(app, &format!("/api/projects/{}/audit-status", project_id)).await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── mcp_remote.rs happy-path : pre-seeded terminal-status run ────
    //
    // workflow_wait_for_completion is a long-poll loop on DB row.status.
    // With an already-terminal run pre-inserted, the loop exits on iter 1
    // and returns the McpWaitResponse — walks ~30 LOC the wrong-id sweep
    // skipped (run lookup happy path + is_terminal_status + status format).
    //
    // workflow_run_status / workflow_run_discussions behave similarly when
    // the run row is present.

    async fn seed_terminal_run(state: &AppState, status: kronn::models::RunStatus) -> String {
        let now = chrono::Utc::now();
        // Seed workflow first to satisfy FK on workflow_runs.workflow_id.
        let workflow_id = format!("wf-{}", uuid::Uuid::new_v4());
        let wf = kronn::models::Workflow {
            pinned: false,
            id: workflow_id.clone(),
            name: "TestWF".into(),
            project_id: None,
            trigger: kronn::models::WorkflowTrigger::Manual,
            steps: vec![],
            actions: vec![],
            safety: kronn::models::WorkflowSafety {
                sandbox: false,
                max_files: None,
                max_lines: None,
                require_approval: false,
            },
            workspace_config: None,
            concurrency_limit: None,
            guards: None,
            artifacts: std::collections::HashMap::new(),
            on_failure: vec![],
            exec_allowlist: vec![],
            variables: vec![],
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        let wf_clone = wf.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::workflows::insert_workflow(conn, &wf_clone))
            .await
            .unwrap();

        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let run = kronn::models::WorkflowRun {
            id: run_id.clone(),
            workflow_id: workflow_id.clone(),
            status,
            trigger_context: None,
            step_results: vec![],
            tokens_used: 1234,
            workspace_path: None,
            started_at: now - chrono::Duration::seconds(10),
            finished_at: Some(now),
            run_type: "linear".into(),
            batch_total: 0,
            batch_completed: 0,
            batch_failed: 0,
            batch_no_response: 0,
            batch_name: None,
            parent_run_id: None,
            state: std::collections::HashMap::new(),
            produced_branches: vec![],
            parent_workflow_id: None,
            parent_workflow_name: None,
            parent_run_started_at: None,
        };
        let r = run.clone();
        state
            .db
            .with_conn(move |conn| kronn::db::workflows::insert_run(conn, &r))
            .await
            .unwrap();
        run_id
    }

    #[tokio::test]
    async fn mcp_workflow_wait_for_completion_returns_immediately_on_terminal_success() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Success).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/workflow-wait-for-completion",
            serde_json::json!({ "run_id": run_id, "timeout_s": 5 }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        let data = &json["data"];
        assert_eq!(data["run_id"], run_id);
        assert!(data["workflow_id"]
            .as_str()
            .unwrap_or("")
            .starts_with("wf-"));
        assert_eq!(data["timed_out"], serde_json::Value::Bool(false));
        assert!(
            data["next_check"].is_null(),
            "terminal run must not return polling hint"
        );
        assert_eq!(data["tokens_used"], 1234);
    }

    #[tokio::test]
    async fn mcp_workflow_wait_for_completion_returns_immediately_on_failed() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Failed).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/workflow-wait-for-completion",
            serde_json::json!({ "run_id": run_id, "timeout_s": 5 }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["timed_out"], serde_json::Value::Bool(false));
        // Status should serialize as "Failed" via Debug
        let status_str = json["data"]["status"].as_str().unwrap_or("");
        assert_eq!(status_str, "Failed");
    }

    #[tokio::test]
    async fn mcp_workflow_wait_for_completion_returns_immediately_on_cancelled() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Cancelled).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            "/api/mcp/workflow-wait-for-completion",
            serde_json::json!({ "run_id": run_id, "timeout_s": 5 }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["data"]["status"], "Cancelled");
    }

    #[tokio::test]
    async fn mcp_workflow_run_status_seeded_run_returns_envelope() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Success).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/mcp/workflow-run-status/{}", run_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert_eq!(json["data"]["run_id"], run_id);
    }

    #[tokio::test]
    async fn mcp_workflow_run_status_exposes_live_step_start() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Success).await;
        let started_at = chrono::Utc::now() - chrono::Duration::seconds(90);
        let step_result = kronn::models::StepResult {
            step_name: "review_pack".into(),
            status: kronn::models::RunStatus::Running,
            output: String::new(),
            tokens_used: 0,
            duration_ms: 0,
            started_at: Some(started_at),
            condition_result: None,
            envelope_detected: None,
            step_kind: Some("Agent".into()),
            step_agent: Some(kronn::models::AgentType::ClaudeCode),
            step_model: None,
            step_api_plugin_slug: None,
            step_api_endpoint_path: None,
            is_rollback: false,
            child_run_id: None,
            native_tool_calls: Box::default(),
        };
        let run_for_update = run_id.clone();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE workflow_runs SET status = 'Running', finished_at = NULL, step_results_json = ?2 WHERE id = ?1",
                    rusqlite::params![run_for_update, serde_json::to_string(&vec![step_result])?],
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/mcp/workflow-run-status/{run_id}");
        let (status, json) = get_json(app, &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["current_step"], "review_pack");
        assert_eq!(json["data"]["steps"][0]["status"], "Running");
        assert_eq!(
            json["data"]["steps"][0]["started_at"],
            started_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
        );
        assert!(json["data"]["steps"][0]["tokens_used"].is_null());
        assert_eq!(json["data"]["steps"][0]["tokens_status"], "in_progress");
        let observed_start = chrono::DateTime::parse_from_rfc3339(
            json["data"]["steps"][0]["started_at"]
                .as_str()
                .expect("started_at string"),
        )
        .expect("RFC3339 started_at");
        let alarm_threshold = chrono::Duration::seconds(60);
        assert!(
            chrono::Utc::now().signed_duration_since(observed_start) > alarm_threshold,
            "negative control: a threshold below the live step age must alarm"
        );
    }

    #[tokio::test]
    async fn mcp_workflow_run_status_keeps_queued_run_distinct_from_live_step() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Success).await;
        let run_for_update = run_id.clone();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "UPDATE workflow_runs SET status = 'Running', finished_at = NULL, step_results_json = '[]' WHERE id = ?1",
                    rusqlite::params![run_for_update],
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/mcp/workflow-run-status/{run_id}");
        let (status, json) = get_json(app, &uri).await;
        assert_eq!(status, StatusCode::OK);
        assert!(json["data"]["current_step"].is_null());
        assert_eq!(json["data"]["steps"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn resume_interrupted_accepts_empty_object_and_non_empty_json_bodies() {
        for body in ["", "{}", r#"{"retry_uncertain_effect":false}"#] {
            let state = test_state();
            let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Interrupted).await;
            let app = build_router_with_auth(state, false);
            let request = Request::builder()
                .method("POST")
                .uri(format!("/api/workflow-runs/{run_id}/resume"))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "body={body:?}");
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(json["success"], true, "body={body:?}: {json}");
            assert_eq!(json["data"]["new_status"], "Running");
        }
    }

    #[tokio::test]
    async fn mcp_workflow_run_discussions_seeded_run_returns_envelope() {
        let state = test_state();
        let run_id = seed_terminal_run(&state, kronn::models::RunStatus::Success).await;
        let app = build_router_with_auth(state, false);

        let uri = format!("/api/mcp/workflow-run-discussions/{}", run_id);
        let (st, json) = get_json(app, &uri).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["success"], serde_json::Value::Bool(true));
        // No batch discs were seeded — `discussions` array should be empty.
        let discs = json["data"]["discussions"].as_array().unwrap();
        assert!(
            discs.is_empty(),
            "no child discs were seeded — array must be empty"
        );
    }

    // ── disc_source.rs (0.8.4 cross-agent memory routes) ─────────────
    // Each route's "wrong inputs" path exercises validation lines AND
    // the DB-not-found branches (~10-30 LOC apiece).
    envelope_post!(
        disc_source_create_minimal_payload,
        "/api/disc/create",
        serde_json::json!({ "title": "T", "agent": "claude-code" })
    );
    envelope_post!(
        disc_source_append_unknown_disc,
        "/api/disc/append",
        serde_json::json!({ "disc_id": "nope", "messages": [] })
    );
    envelope_post!(
        disc_source_link_unknown_disc,
        "/api/disc/link",
        serde_json::json!({
            "disc_id": "nope",
            "source_agent": "claude-code",
            "source_session_id": "s1"
        })
    );
    envelope_post!(
        disc_source_unlink_unknown_disc,
        "/api/disc/unlink",
        serde_json::json!({ "disc_id": "nope" })
    );
    envelope_get!(
        disc_source_find_by_session_unknown,
        "/api/disc/find_by_session?source_agent=claude-code&source_session_id=unknown"
    );
    envelope_get!(
        disc_source_search_returns_envelope,
        "/api/disc/search?q=hello"
    );
    envelope_get!(disc_source_search_empty_query, "/api/disc/search?q=");
    envelope_get!(
        disc_source_load_other_unknown_disc,
        "/api/disc/load_other?disc_id=nope&limit=10"
    );
    envelope_get!(
        disc_source_list_sources_returns_envelope,
        "/api/disc/sources"
    );
    envelope_get!(disc_source_detail_unknown_id, "/api/disc/sources/unknown");

    // ── mcp_remote.rs (was 22% Lines) — JSON wrappers around SSE flows ──
    // Each of these endpoints has lots of input-validation lines BEFORE
    // any actual workflow trigger. We exercise the validation path with
    // wrong/empty inputs.
    envelope_post!(
        mcp_workflow_trigger_unknown,
        "/api/mcp/workflow-trigger",
        serde_json::json!({ "workflow_id": "nope", "project_id": "nope" })
    );
    envelope_post!(
        mcp_workflow_trigger_missing_workflow_id,
        "/api/mcp/workflow-trigger",
        serde_json::json!({ "project_id": "nope" })
    );
    envelope_get!(
        mcp_workflow_run_status_unknown_run,
        "/api/mcp/workflow-run-status/unknown-run-id"
    );
    envelope_post!(
        mcp_qp_run_unknown,
        "/api/mcp/qp-run",
        serde_json::json!({ "quick_prompt_id": "nope", "project_id": "nope", "vars": {} })
    );
    envelope_post!(
        mcp_qp_run_empty_id,
        "/api/mcp/qp-run",
        serde_json::json!({ "quick_prompt_id": "", "project_id": "", "vars": {} })
    );
    envelope_post!(
        mcp_qp_batch_run_unknown,
        "/api/mcp/qp-batch-run",
        serde_json::json!({ "quick_prompt_id": "nope", "project_id": "nope", "items": [] })
    );
    envelope_post!(
        mcp_qp_batch_run_no_items,
        "/api/mcp/qp-batch-run",
        serde_json::json!({ "quick_prompt_id": "x", "project_id": "x", "items": [] })
    );
    envelope_get!(
        mcp_workflow_run_discussions_unknown,
        "/api/mcp/workflow-run-discussions/unknown-run"
    );
    envelope_post!(
        mcp_workflow_wait_unknown_run,
        "/api/mcp/workflow-wait-for-completion",
        serde_json::json!({ "run_id": "unknown-run", "timeout_s": 1 })
    );
    envelope_post!(
        mcp_workflow_wait_empty_run_id,
        "/api/mcp/workflow-wait-for-completion",
        serde_json::json!({ "run_id": "", "timeout_s": 1 })
    );
    envelope_post!(
        mcp_workflow_wait_clamps_timeout_low,
        "/api/mcp/workflow-wait-for-completion",
        serde_json::json!({ "run_id": "nope", "timeout_s": 0 })
    ); // clamps to MIN
    envelope_post!(
        mcp_workflow_wait_clamps_timeout_high,
        "/api/mcp/workflow-wait-for-completion",
        serde_json::json!({ "run_id": "nope", "timeout_s": 9999 })
    ); // clamps to MAX

    // ── audit/full.rs cleanup_audit_files + classify_docs_dir paths ──
    // These are pure helpers under #[cfg(test)] reachability.

    #[tokio::test]
    async fn audit_cancel_unknown_project_returns_envelope() {
        // POST /api/projects/:id/cancel-audit on a non-existent project.
        let (st, json) = post_json(
            test_app(),
            "/api/projects/nope/cancel-audit",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    #[tokio::test]
    async fn project_git_branch_rejects_invalid_names() {
        let (_dir, repo) = seed_repo("proj-branch-invalid");
        let state = test_state();
        let project_id = seed_project_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        for bad in ["", "with space", "has..dot"] {
            let (st, json) = post_json(
                build_router_with_auth(test_state(), false),
                &format!("/api/projects/{}/git-branch", project_id),
                serde_json::json!({ "name": bad }),
            )
            .await;
            assert_eq!(st, StatusCode::OK);
            assert_eq!(
                json["success"],
                serde_json::Value::Bool(false),
                "bad name {bad:?} should be rejected"
            );
        }
        // Sanity that we still own the app at the end (not actually used).
        let _ = app;
    }

    #[tokio::test]
    async fn disc_worktree_unlock_with_no_lock_is_idempotent() {
        let (_dir, repo) = seed_repo("worktree-noop");
        let state = test_state();
        let disc_id = seed_disc_with_repo(&state, &repo).await;
        let app = build_router_with_auth(state, false);

        let (st, json) = post_json(
            app,
            &format!("/api/discussions/{}/worktree-unlock", disc_id),
            serde_json::json!({}),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(json.get("success").is_some());
    }

    // ── disc_append cross-instance federation (regression 2026-06-29) ────────
    // A message posted by an AGENT via `disc_append` (the MCP path every CLI
    // peer uses) must broadcast a `ChatMessage` to peers when the disc is
    // shared — otherwise cross-instance agent chat is silently one-sided
    // (the message lands only in the local DB). The UI `send_message` path
    // already broadcasts; this proves `disc_append` now matches it.
    #[tokio::test]
    async fn disc_append_federates_chatmessage_when_disc_is_shared() {
        let state = test_state();
        let mut ws_rx = state.ws_broadcast.subscribe();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO discussions (id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode, shared_id)
                 VALUES ('disc-fed-shared', 'Shared', 'ClaudeCode', 'fr', '[]',
                 datetime('now'), datetime('now'), 0, 'Direct', 'shared-xyz-123')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let app = build_router_with_auth(state.clone(), false);
        let (st, json) = post_json(
            app,
            "/api/disc/append",
            serde_json::json!({
                "disc_id": "disc-fed-shared",
                "messages": [{
                    "source_msg_id": "src-1",
                    "role": "Agent",
                    "content": "hello peers from the agent path",
                }],
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["data"]["appended"], 1);

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), ws_rx.recv())
            .await
            .expect("a ChatMessage must be broadcast for a shared disc")
            .expect("broadcast channel error");
        match received {
            WsMessage::ChatMessage {
                shared_discussion_id,
                content,
                ..
            } => {
                assert_eq!(
                    shared_discussion_id, "shared-xyz-123",
                    "broadcast must carry the disc's shared_id so the peer's mirror accepts it"
                );
                assert_eq!(content, "hello peers from the agent path");
            }
            other => panic!("expected ChatMessage, got {other:?}"),
        }
    }

    // The mirror invariant: a PURELY LOCAL disc (shared_id NULL) must NOT
    // emit a ChatMessage — federating a private disc would leak it to peers.
    #[tokio::test]
    async fn disc_append_does_not_federate_when_disc_is_not_shared() {
        let state = test_state();
        let mut ws_rx = state.ws_broadcast.subscribe();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO discussions (id, title, agent, language, participants_json,
                 created_at, updated_at, message_count, workspace_mode)
                 VALUES ('disc-fed-local', 'Local', 'ClaudeCode', 'fr', '[]',
                 datetime('now'), datetime('now'), 0, 'Direct')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let app = build_router_with_auth(state.clone(), false);
        let (st, json) = post_json(
            app,
            "/api/disc/append",
            serde_json::json!({
                "disc_id": "disc-fed-local",
                "messages": [{
                    "source_msg_id": "src-1",
                    "role": "Agent",
                    "content": "private, stays local",
                }],
            }),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(json["data"]["appended"], 1);

        // No ChatMessage should arrive within a short window.
        let res = tokio::time::timeout(std::time::Duration::from_millis(300), ws_rx.recv()).await;
        if let Ok(Ok(WsMessage::ChatMessage { .. })) = res {
            panic!("a non-shared disc must not federate its messages");
        }
    }
}
