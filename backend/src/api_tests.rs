/// Integration tests for the HTTP API layer.
///
/// Each test spins up a real Axum router backed by an in-memory SQLite database,
/// sends HTTP requests via `tower::ServiceExt::oneshot`, and asserts on the JSON
/// responses — exactly the same way Axum's own examples do it.
#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt; // for `oneshot`

    use crate::{
        build_router_with_auth, core::config::default_config, db::Database, AppState,
        DEFAULT_MAX_CONCURRENT_AGENTS,
    };
    use serial_test::serial;

    // ─── Helper: build a test AppState with an in-memory DB ──────────────────

    /// Handlers under test call config::save — without KRONN_DATA_DIR that
    /// writes the developer's REAL config.toml (2026-07-13 incident; the
    /// persist_atomic guard now panics instead).
    fn isolate_config_dir() {
        // Called ONLY by the #[serial] tests whose handlers SAVE config —
        // a global call from test_state() would mutate KRONN_DATA_DIR from
        // 80 non-serial tests and race the serialized env family. Re-set on
        // EVERY call (same stable path, so repeats are idempotent): the
        // config.rs #[serial] env tests legitimately remove_var at their
        // end, and a Once guard left later callers with no dir at all —
        // the write-guard panic fired on whichever test ran after them.
        let dir = std::env::temp_dir().join(format!("kronn-libtest-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        std::env::set_var("KRONN_DATA_DIR", &dir);
    }

    fn test_state() -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config_arc = Arc::new(RwLock::new(default_config()));
        AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Build a test AppState with a specific auth token configured.
    fn test_state_with_token(token: &str) -> AppState {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let mut config = default_config();
        config.server.auth_token = Some(token.to_string());
        config.server.auth_enabled = true;
        let config_arc = Arc::new(RwLock::new(config));
        AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    /// Send a request and collect the response body as parsed JSON.
    async fn send(state: AppState, enable_auth: bool, req: Request<Body>) -> (StatusCode, Value) {
        let app = build_router_with_auth(state, enable_auth);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    // ─── GET /api/resolve/:id ───────────────────────────────────────────────

    #[tokio::test]
    async fn resolve_id_returns_compact_message_context() {
        let state = test_state();
        state
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO discussions
                     (id, title, agent, language, created_at, updated_at)
                     VALUES ('disc-resolve', 'Resolver room', 'Codex', 'fr', 'now', 'now')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO messages
                     (id, discussion_id, role, content, timestamp, sort_order)
                     VALUES ('message-resolve', 'disc-resolve', 'User',
                             'Which Kronn object is this?', 'now', 1)",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let request = Request::builder()
            .uri("/api/resolve/message-resolve")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state, false, request).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert_eq!(json["data"]["kind"], "message");
        assert_eq!(json["data"]["parent"]["id"], "disc-resolve");
        assert_eq!(json["data"]["suggested_tool"], "disc_get_message");
        assert_eq!(json["data"]["summary"], "Which Kronn object is this?");
    }

    #[tokio::test]
    async fn resolve_id_returns_typed_not_found_envelope() {
        let request = Request::builder()
            .uri("/api/resolve/unknown-object")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(test_state(), false, request).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], false);
        assert_eq!(json["error_code"], "not_found");
    }

    #[tokio::test]
    async fn resolve_id_routes_agent_library_ids_without_exposing_prompt_bodies() {
        let cases = [
            ("security", "skill", "skill_get"),
            ("architect", "profile", "profile_get"),
            ("token-saver", "directive", "directive_get"),
        ];

        for (id, kind, suggested_tool) in cases {
            let request = Request::builder()
                .uri(format!("/api/resolve/{id}"))
                .body(Body::empty())
                .unwrap();
            let (status, json) = send(test_state(), false, request).await;

            assert_eq!(status, StatusCode::OK);
            assert_eq!(json["success"], true, "failed to resolve {id}");
            assert_eq!(json["data"]["kind"], kind);
            assert_eq!(json["data"]["suggested_tool"], suggested_tool);
            let wire = json["data"].to_string();
            assert!(!wire.contains("persona_prompt"));
            assert!(!wire.contains("content"));
        }
    }

    #[tokio::test]
    async fn resolve_id_does_not_reveal_a_locked_secret_profile() {
        let locked_request = Request::builder()
            .uri("/api/resolve/batman")
            .body(Body::empty())
            .unwrap();
        let (_, locked_json) = send(test_state(), false, locked_request).await;
        assert_eq!(locked_json["success"], false);
        assert_eq!(locked_json["error_code"], "not_found");

        let unlocked_state = test_state();
        unlocked_state
            .config
            .write()
            .await
            .unlocked_profiles
            .push("batman".into());
        let unlocked_request = Request::builder()
            .uri("/api/resolve/batman")
            .body(Body::empty())
            .unwrap();
        let (_, unlocked_json) = send(unlocked_state, false, unlocked_request).await;
        assert_eq!(unlocked_json["success"], true);
        assert_eq!(unlocked_json["data"]["kind"], "profile");
        assert_eq!(unlocked_json["data"]["suggested_tool"], "profile_get");
    }

    #[tokio::test]
    async fn resolve_id_returns_typed_conflict_for_cross_registry_collision() {
        let state = test_state();
        state
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, created_at, updated_at)
                     VALUES ('security', 'Collision', '/tmp/collision', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let request = Request::builder()
            .uri("/api/resolve/security")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state, false, request).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], false);
        assert_eq!(json["error_code"], "conflict");
        assert!(json["error"].as_str().unwrap().contains("project, skill"));
    }

    #[tokio::test]
    async fn discussion_notes_endpoint_is_bounded_paginated_and_note_only() {
        let state = test_state();
        state
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO discussions
                     (id, title, agent, language, created_at, updated_at)
                     VALUES ('disc-notes', 'Notes room', 'Codex', 'fr', 'now', 'now')",
                    [],
                )?;
                connection.execute_batch(
                    "INSERT INTO messages
                         (id, discussion_id, role, channel, content, timestamp, sort_order)
                     VALUES
                         ('note-1', 'disc-notes', 'User', 'note', 'Première note', 'now', 1),
                         ('main-2', 'disc-notes', 'User', 'main', 'Message normal', 'now', 2),
                         ('note-3', 'disc-notes', 'Agent', 'note', 'Deuxième note', 'now', 3);",
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let first = Request::builder()
            .uri("/api/discussions/disc-notes/notes?limit=1")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, first).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["total_notes"], 2);
        assert_eq!(json["data"]["notes"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"]["notes"][0]["message"]["id"], "note-1");
        assert_eq!(json["data"]["notes"][0]["message"]["channel"], "note");
        assert_eq!(
            json["data"]["notes"][0]["attachments"],
            serde_json::json!([])
        );
        assert_eq!(json["data"]["next_cursor"], 1);

        let second = Request::builder()
            .uri("/api/discussions/disc-notes/notes?limit=1&cursor=1")
            .body(Body::empty())
            .unwrap();
        let (_, json) = send(state, false, second).await;
        assert_eq!(json["data"]["notes"][0]["message"]["id"], "note-3");
        assert!(json["data"].get("next_cursor").is_none());
    }

    // ─── GET /api/discussions/running (2026-06-24) ────────────────────────────

    /// Surfaces in-flight runs so a background/batch agent still working after
    /// you navigate away keeps showing as running (no needless re-launch).
    /// Pins: the static `/running` route isn't swallowed by `/{id}`, the
    /// response reflects the cancel registry, and the `CancelGuard` Drop clears
    /// the entry (the RAII that guarantees no ghost "running" state).
    #[tokio::test]
    async fn running_discussions_reflects_registry_and_clears_on_drop() {
        let state = test_state();
        let get = || {
            Request::builder()
                .uri("/api/discussions/running")
                .body(Body::empty())
                .unwrap()
        };

        let (status, json) = send(state.clone(), false, get()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert_eq!(
            json["data"].as_array().unwrap().len(),
            0,
            "nothing running initially"
        );

        {
            let _g =
                crate::CancelGuard::insert(&state.cancel_registry, "disc-running-1".to_string());
            let (_s, json) = send(state.clone(), false, get()).await;
            let ids: Vec<&str> = json["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert_eq!(ids, vec!["disc-running-1"], "in-flight run must be listed");
        } // CancelGuard dropped here → registry entry removed

        let (_s, json) = send(state.clone(), false, get()).await;
        assert_eq!(
            json["data"].as_array().unwrap().len(),
            0,
            "CancelGuard Drop must clear the running entry"
        );
    }

    #[tokio::test]
    async fn git_switch_refuses_an_active_direct_discussion_and_names_it() {
        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-git-switch', 'Switch guard project', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                connection.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at,
                      workspace_mode)
                     VALUES ('disc-direct-running', 'project-git-switch',
                             'Release preparation', 'Codex', 'en', 'now', 'now', 'Direct')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("seed project and discussion");

        let _run =
            crate::CancelGuard::insert(&state.cancel_registry, "disc-direct-running".to_string());
        let request = Request::builder()
            .method("POST")
            .uri("/api/projects/project-git-switch/git-switch")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"branch":"feature/unsafe-switch"}"#))
            .unwrap();

        let (status, json) = send(state, false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], false);
        let error = json["error"].as_str().expect("explicit switch error");
        assert!(error.contains("Release preparation"), "{error}");
        assert!(error.contains("disc-direct-running"), "{error}");
        assert!(error.contains("stop it explicitly"), "{error}");
    }

    /// KT-89 — test mode is a SECOND kind of root occupancy. It requires a
    /// worktree, so the discussion is `Isolated` and the running-run filter of
    /// KT-71 cannot see it; switching underneath it would move the branch out
    /// from under `test_mode_restore_branch`. No active run here on purpose: the
    /// durable state alone must be enough to refuse.
    #[tokio::test]
    async fn git_switch_refuses_while_test_mode_holds_the_root() {
        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-test-mode', 'Test mode project', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                connection.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at,
                      workspace_mode, worktree_branch, test_mode_restore_branch)
                     VALUES ('disc-in-test-mode', 'project-test-mode',
                             'Fastly drawer review', 'ClaudeCode', 'fr', 'now', 'now',
                             'Isolated', 'feat/drawer', 'main')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("seed project and test-mode discussion");

        let request = Request::builder()
            .method("POST")
            .uri("/api/projects/project-test-mode/git-switch")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"branch":"feature/unsafe-switch"}"#))
            .unwrap();

        let (status, json) = send(state, false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], false);
        let error = json["error"].as_str().expect("explicit switch error");
        assert!(error.contains("test mode"), "must name the cause: {error}");
        assert!(error.contains("Fastly drawer review"), "{error}");
        assert!(error.contains("disc-in-test-mode"), "{error}");
        assert!(
            error.contains("Leave test mode"),
            "must say how to get out: {error}"
        );
    }

    /// The guard must not fire on a discussion that merely HAS a worktree: only
    /// an active test mode holds the root.
    #[tokio::test]
    async fn git_switch_ignores_an_isolated_discussion_not_in_test_mode() {
        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-isolated', 'Isolated project', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                connection.execute(
                    "INSERT INTO discussions
                     (id, project_id, title, agent, language, created_at, updated_at,
                      workspace_mode, worktree_branch)
                     VALUES ('disc-isolated-idle', 'project-isolated',
                             'Background refactor', 'Codex', 'en', 'now', 'now',
                             'Isolated', 'feat/refactor')",
                    [],
                )?;
                Ok(())
            })
            .await
            .expect("seed project and isolated discussion");

        let request = Request::builder()
            .method("POST")
            .uri("/api/projects/project-isolated/git-switch")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"branch":"feature/whatever"}"#))
            .unwrap();

        let (status, json) = send(state, false, request).await;
        assert_eq!(status, StatusCode::OK);
        // The switch itself fails (the temp dir is not a git repo), but it must
        // NOT fail with the test-mode refusal.
        if let Some(error) = json["error"].as_str() {
            assert!(
                !error.contains("test mode"),
                "an idle worktree must not be mistaken for test mode: {error}"
            );
        }
    }

    /// KT-94 — the Git status must not wait for the language statistics. Cold
    /// cache: the response arrives with an honest empty bar, and the languages
    /// land in the cache afterwards, off the response path.
    #[tokio::test]
    async fn git_status_answers_before_the_language_stats_are_computed() {
        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        // A real file, so the background computation has something to find.
        std::fs::write(project_dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-lang-async', 'Lang async', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                Ok(())
            })
            .await
            .expect("seed project");

        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "t@t.t"],
            vec!["config", "user.name", "T"],
            vec!["add", "."],
            vec!["commit", "-m", "seed"],
        ] {
            let out = std::process::Command::new("git")
                .args(&args)
                .current_dir(project_dir.path())
                .output()
                .expect("git command");
            assert!(out.status.success(), "git {args:?} failed");
        }

        let request = Request::builder()
            .method("GET")
            .uri("/api/projects/project-lang-async/git-status")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true, "{json}");
        // The first answer must NOT have waited for the scan: empty bar, null
        // timestamp — an honest "not computed yet", never a block.
        // The serializer omits empty vecs, so absent == empty.
        assert!(
            json["data"]["languages"]
                .as_array()
                .is_none_or(|languages| languages.is_empty()),
            "{json}"
        );
        assert!(json["data"]["languages_checked_at"].is_null(), "{json}");

        // The background task then fills the cache without any further request
        // blocking on it.
        let mut cached = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if state
                .git_language_cache
                .lock()
                .await
                .get("project-lang-async")
                .is_some()
            {
                cached = true;
                break;
            }
        }
        assert!(cached, "the language stats must land in the cache off-path");

        // And the next call serves them as cached.
        let request = Request::builder()
            .method("GET")
            .uri("/api/projects/project-lang-async/git-status")
            .body(Body::empty())
            .unwrap();
        let (_, json) = send(state, false, request).await;
        assert_eq!(json["data"]["languages_cached"], true, "{json}");
    }

    // ─── Q1: Workflow execution integration test ──────────────────────────────

    /// Create a workflow, trigger it, and verify a run is recorded.
    ///
    /// Because no real agent binary is available in tests, the run ends with
    /// `Failed` status (or stays `Pending` if the runner exits immediately).
    /// Either outcome proves the full create→trigger→list-runs path works.
    #[tokio::test]
    async fn workflow_create_trigger_and_list_runs() {
        let state = test_state();

        // 1. Create a workflow via POST /api/workflows
        // WorkflowTrigger and StepMode use #[serde(tag = "type")], so
        // "Manual" → { "type": "Manual" }, "Normal" → { "type": "Normal" }.
        let create_body = serde_json::json!({
            "name": "Test Integration Workflow",
            "trigger": { "type": "Manual" },
            "steps": [
                {
                    "name": "step1",
                    "agent": "ClaudeCode",
                    "prompt_template": "Say hello",
                    "mode": { "type": "Normal" }
                }
            ],
            "actions": [],
            "safety": {
                "sandbox": false,
                "max_files": null,
                "max_lines": null,
                "require_approval": false
            }
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();

        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "create workflow: {body}");
        assert!(
            body["success"].as_bool().unwrap_or(false),
            "create workflow ok: {body}"
        );

        let workflow_id = body["data"]["id"]
            .as_str()
            .expect("workflow id")
            .to_string();
        assert!(!workflow_id.is_empty(), "workflow id should not be empty");

        // 2. Trigger the workflow via POST /api/workflows/{id}/trigger
        //    The trigger endpoint returns SSE — we fire-and-forget it,
        //    then check the runs list.  We give the background task a brief moment
        //    to insert the run record before we query.
        let trigger_req = Request::builder()
            .method("POST")
            .uri(format!("/api/workflows/{}/trigger", workflow_id))
            .body(Body::empty())
            .unwrap();

        let app = build_router_with_auth(state.clone(), false);
        let trigger_resp = app.oneshot(trigger_req).await.expect("trigger oneshot");
        // SSE always returns 200, even if execution later fails
        assert_eq!(
            trigger_resp.status(),
            StatusCode::OK,
            "trigger should return 200 (SSE)"
        );

        // Consume the SSE body so the background task completes
        let _ = trigger_resp.into_body().collect().await;

        // Small sleep to let the spawned runner task update the DB
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // 3. List runs via GET /api/workflows/{id}/runs
        let list_req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}/runs", workflow_id))
            .body(Body::empty())
            .unwrap();

        let (status, runs_body) = send(state.clone(), false, list_req).await;
        assert_eq!(status, StatusCode::OK, "list runs: {runs_body}");
        assert!(
            runs_body["success"].as_bool().unwrap_or(false),
            "list runs ok: {runs_body}"
        );

        let runs = runs_body["data"].as_array().expect("runs array");
        assert!(
            !runs.is_empty(),
            "at least one run should exist after trigger"
        );

        // 4. The run status must be Pending, Failed, or Success.
        //    (No real agent binary is available in tests; the runner may fast-fail
        //    or complete immediately depending on the environment.)
        let run_status = runs[0]["status"].as_str().expect("run status");
        assert!(
            run_status == "Pending" || run_status == "Failed" || run_status == "Success",
            "expected Pending, Failed, or Success, got: {run_status}"
        );
    }

    #[tokio::test]
    async fn workflow_runs_endpoint_paginates_large_histories() {
        let state = test_state();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO workflows
                     (id, name, trigger_json, steps_json, actions_json, safety_json,
                      enabled, created_at, updated_at)
                     VALUES ('wf-page', 'Paged', '{\"type\":\"Manual\"}', '[]', '[]',
                             '{}', 1, '2026-07-24T09:00:00Z', '2026-07-24T09:00:00Z')",
                    [],
                )?;
                for index in 1..=4 {
                    conn.execute(
                        "INSERT INTO workflow_runs
                         (id, workflow_id, status, step_results_json, started_at)
                         VALUES (?1, 'wf-page', 'Success', '[]', ?2)",
                        rusqlite::params![
                            format!("run-{index}"),
                            format!("2026-07-24T09:0{index}:00Z")
                        ],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let req = Request::builder()
            .method("GET")
            .uri("/api/workflows/wf-page/runs?limit=2&offset=1")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;

        assert_eq!(status, StatusCode::OK);
        let ids: Vec<&str> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| run["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["run-3", "run-2"]);

        let count_req = Request::builder()
            .method("GET")
            .uri("/api/workflows/wf-page/runs/count")
            .body(Body::empty())
            .unwrap();
        let (count_status, count_body) = send(state, false, count_req).await;
        assert_eq!(count_status, StatusCode::OK);
        assert_eq!(count_body["data"], 4);
    }

    #[tokio::test]
    async fn page_publications_endpoint_returns_only_three_newest_refreshes() {
        let state = test_state();
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO workflows
                     (id, name, trigger_json, steps_json, actions_json, safety_json,
                      enabled, created_at, updated_at)
                     VALUES ('wf-refresh', 'Adobe refresh', '{\"type\":\"Manual\"}', '[]', '[]',
                             '{}', 1, '2026-08-14T08:00:00Z', '2026-08-14T08:00:00Z')",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO live_pages
                     (id, title, slug, data_revision, created_at, updated_at, last_published_at)
                     VALUES ('page-refresh', 'Adobe signals', 'adobe-signals', 4,
                             '2026-08-14T08:00:00Z', '2026-08-14T08:04:00Z',
                             '2026-08-14T08:04:00Z')",
                    [],
                )?;
                for revision in 1..=4 {
                    conn.execute(
                        "INSERT INTO live_page_publications
                         (id, page_id, data_revision, workflow_id, datasets_json,
                          changed_datasets_json, unchanged_datasets_json,
                          points_added, points_removed, published_at)
                         VALUES (?1, 'page-refresh', ?2, 'wf-refresh', '[\"summary\"]',
                                 ?3, ?4, 0, 0, ?5)",
                        rusqlite::params![
                            format!("publication-{revision}"),
                            revision,
                            if revision % 2 == 0 {
                                "[\"summary\"]"
                            } else {
                                "[]"
                            },
                            if revision % 2 == 0 {
                                "[]"
                            } else {
                                "[\"summary\"]"
                            },
                            format!("2026-08-14T08:0{revision}:00Z"),
                        ],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let request = Request::builder()
            .uri("/api/pages/adobe-signals/publications")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, request).await;

        assert_eq!(status, StatusCode::OK);
        let publications = json["data"].as_array().unwrap();
        assert_eq!(publications.len(), 3);
        assert_eq!(publications[0]["data_revision"], 4);
        assert_eq!(publications[2]["data_revision"], 2);
        assert_eq!(publications[0]["workflow_id"], "wf-refresh");
        assert_eq!(publications[0]["workflow_name"], "Adobe refresh");
        assert_eq!(
            publications[0]["datasets_updated"],
            serde_json::json!(["summary"])
        );
        assert_eq!(publications[0]["content_changed"], true);
        assert_eq!(
            publications[0]["changed_datasets"],
            serde_json::json!(["summary"])
        );
        assert_eq!(publications[1]["content_changed"], false);
        assert_eq!(
            publications[1]["unchanged_datasets"],
            serde_json::json!(["summary"])
        );

        let missing_request = Request::builder()
            .uri("/api/pages/missing/publications")
            .body(Body::empty())
            .unwrap();
        let (_, missing_json) = send(state, false, missing_request).await;
        assert_eq!(missing_json["success"], false);
        assert_eq!(missing_json["error_code"], "not_found");
    }

    // ─── Q2: Auth middleware tests ────────────────────────────────────────────

    /// Health endpoint bypasses auth even when auth is enabled.
    #[tokio::test]
    async fn auth_health_bypasses_auth() {
        let state = test_state_with_token("secret-test-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "GET /api/health should return 200 even with auth enabled"
        );
    }

    /// `/api/health` exposes `in_docker` (a bool) so the UI can gate the
    /// agent Install button — installs land in the container under Docker, so
    /// the UI must point to the host-side CLI instead. Health is unauthed.
    #[tokio::test]
    async fn health_exposes_in_docker_bool() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, true, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.get("in_docker")
                .map(|v| v.is_boolean())
                .unwrap_or(false),
            "health must expose a boolean `in_docker`, got: {body}"
        );
    }

    /// A request without an Authorization header returns 401 when auth is enabled.
    #[tokio::test]
    async fn auth_missing_header_returns_401() {
        let state = test_state_with_token("secret-test-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "missing auth header should return 401"
        );
    }

    /// A request with the correct Bearer token returns 200.
    #[tokio::test]
    async fn auth_valid_token_returns_200() {
        let token = "my-valid-token";
        let state = test_state_with_token(token);

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .header("Authorization", format!("Bearer {}", token))
            .body(Body::empty())
            .unwrap();

        let (status, body) = send(state, true, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "valid token should return 200: {body}"
        );
    }

    /// Master switch: when `auth_enabled = false`, auth is skipped even if a
    /// token is still configured. This is the supported escape hatch for the
    /// Docker Desktop macOS lockout, where the localhost bypass can't see the
    /// real client IP (every published-port request is NAT'd to the Docker
    /// gateway). Regression guard: a token left in the config must NOT keep
    /// enforcing auth once the user has turned the master switch off.
    #[tokio::test]
    async fn auth_disabled_master_switch_bypasses_even_with_token() {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let mut config = default_config();
        config.server.auth_token = Some("present-but-unused".to_string());
        config.server.auth_enabled = false;
        let config_arc = Arc::new(RwLock::new(config));
        let state = AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS);

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "auth_enabled=false must bypass auth even with a token present",
        );
    }

    /// A request with a wrong token returns 401.
    #[tokio::test]
    async fn auth_wrong_token_returns_401() {
        let state = test_state_with_token("correct-token");

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .header("Authorization", "Bearer wrong-token")
            .body(Body::empty())
            .unwrap();

        let (status, _) = send(state, true, req).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "wrong token should return 401"
        );
    }

    /// WS endpoint skips auth — authentication is handled via invite code in ws.rs.
    #[tokio::test]
    async fn auth_ws_always_accessible() {
        let token = "ws-test-token";
        let state = test_state_with_token(token);

        // WS without token → should NOT be 401 (auth skipped for /api/ws)
        let req = Request::builder()
            .method("GET")
            .uri("/api/ws")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(state, true, req).await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "WS should skip auth middleware — invite code verified in ws.rs"
        );
    }

    // ─── Q3: Projects API integration tests ───────────────────────────────────

    #[tokio::test]
    async fn projects_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projects_crud_lifecycle() {
        let state = test_state();

        // Create a project directly in DB (projects are created via scan, not POST)
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "test-proj".into(),
                    name: "Test Project".into(),
                    path: "/tmp/test-project".into(),
                    repo_url: Some("https://github.com/test/repo".into()),
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        // GET /api/projects — should list it
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        let projects = body["data"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"].as_str().unwrap(), "Test Project");

        // GET /api/projects/:id — should return it
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/test-proj")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["id"].as_str().unwrap(), "test-proj");

        // DELETE /api/projects/:id
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/projects/test-proj")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // GET /api/projects — should be empty now
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn projects_get_nonexistent_returns_error() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/nonexistent-id")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK); // API returns 200 with success=false
        assert!(!body["success"].as_bool().unwrap_or(true));
    }

    // ─── Q4: Config API integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn config_language_get_default() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/language")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // Default language is "fr" (defined in default_config)
        let lang = body["data"].as_str().unwrap();
        assert!(!lang.is_empty(), "Language should have a default value");
    }

    #[tokio::test]
    #[serial]
    async fn config_language_set_and_get() {
        isolate_config_dir();
        let state = test_state();

        // Set language to "en"
        let req = Request::builder()
            .method("POST")
            .uri("/api/config/language")
            .header("Content-Type", "application/json")
            .body(Body::from("\"en\""))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "set language: {body}");

        // Get language — should be "en"
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/language")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"].as_str().unwrap(), "en");
    }

    #[tokio::test]
    #[serial]
    async fn config_ui_language_accepts_chinese_and_rejects_unknown_locales() {
        isolate_config_dir();
        let state = test_state();

        let req = Request::builder()
            .method("POST")
            .uri("/api/config/ui-language")
            .header("Content-Type", "application/json")
            .body(Body::from("\"zh\""))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "set Chinese UI locale: {body}");
        assert_eq!(state.config.read().await.ui_language, "zh");

        let req = Request::builder()
            .method("POST")
            .uri("/api/config/ui-language")
            .header("Content-Type", "application/json")
            .body(Body::from("\"de\""))
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(body["success"], false);
        assert!(body["error"].as_str().unwrap().contains("fr|en|es|zh"));
        assert_eq!(state.config.read().await.ui_language, "zh");
    }

    // ─── Q5: MCP API integration tests ────────────────────────────────────────

    #[tokio::test]
    async fn mcps_overview_returns_servers_and_configs() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/mcps")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // Overview should have servers and configs arrays
        assert!(
            body["data"]["servers"].is_array(),
            "Overview should include servers"
        );
        assert!(
            body["data"]["configs"].is_array(),
            "Overview should include configs"
        );
        let servers = body["data"]["servers"].as_array().unwrap();
        assert!(
            servers.is_empty(),
            "No servers in DB initially (registry is not auto-imported)"
        );
    }

    #[tokio::test]
    async fn mcps_registry_lists_builtin_servers() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/mcps/registry")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let registry = body["data"].as_array().unwrap();
        assert!(
            registry.len() >= 30,
            "Registry should have at least 30 entries, got {}",
            registry.len()
        );
    }

    // ─── Q6: Setup API integration tests ──────────────────────────────────────

    #[tokio::test]
    async fn setup_status_returns_valid_response() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/setup/status")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert!(
            body["data"]["agents_detected"].is_array(),
            "Setup status should include agents_detected"
        );
    }

    // ─── Q7: Stats API ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn stats_token_usage_returns_ok() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/stats/tokens")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── Q8: Discussions API integration tests ────────────────────────────────

    #[tokio::test]
    async fn discussions_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn discussions_create_and_get() {
        let state = test_state();

        // Create discussion (will fail to run agent but should persist in DB)
        let create_body = serde_json::json!({
            "title": "Test Discussion",
            "agent": "ClaudeCode",
            "language": "fr",
            "initial_prompt": "Hello, test prompt"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        // Create returns SSE stream, status should be 200
        assert_eq!(status, StatusCode::OK, "create discussion: {body}");

        // Wait for background task to persist
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // List discussions — should have 1
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discussions = body["data"].as_array().unwrap();
        assert_eq!(
            discussions.len(),
            1,
            "Should have 1 discussion after create"
        );
        let disc_id = discussions[0]["id"].as_str().unwrap().to_string();
        assert_eq!(discussions[0]["title"].as_str().unwrap(), "Test Discussion");
        assert_eq!(discussions[0]["language"].as_str().unwrap(), "fr");

        // Get by ID
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/discussions/{}", disc_id))
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["id"].as_str().unwrap(), disc_id);
        // Should have at least the initial user message
        let messages = body["data"]["messages"].as_array().unwrap();
        assert!(
            !messages.is_empty(),
            "Discussion should have at least 1 message"
        );
        assert_eq!(messages[0]["role"].as_str().unwrap(), "User");
    }

    #[tokio::test]
    async fn discussions_update_title_and_archive() {
        let state = test_state();

        // Create via DB directly (faster, no SSE)
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let disc = crate::models::Discussion {
                    awaiting_agent: false,
                    agent_running: false,
                    id: "disc-1".into(),
                    project_id: None,
                    title: "Original Title".into(),
                    agent: crate::models::AgentType::ClaudeCode,
                    language: "en".into(),
                    participants: vec![crate::models::AgentType::ClaudeCode],
                    message_count: 0,
                    non_system_message_count: 0,
                    messages: vec![],
                    skill_ids: vec![],
                    profile_ids: vec![],
                    directive_ids: vec![],
                    archived: false,
                    pinned: false,
                    workspace_mode: "Direct".into(),
                    workspace_path: None,
                    worktree_branch: None,
                    tier: crate::models::ModelTier::Default,
                    model: None,
                    pin_first_message: false,
                    summary_cache: None,
                    summary_up_to_msg_idx: None,
                    summary_strategy: crate::models::SummaryStrategy::Auto,
                    introspection_call_count: 0,
                    shared_id: None,
                    shared_with: vec![],
                    workflow_run_id: None,
                    test_mode_restore_branch: None,
                    test_mode_stash_ref: None,
                    created_at: now,
                    updated_at: now,
                };
                crate::db::discussions::insert_discussion(conn, &disc)?;
                Ok(())
            })
            .await
            .unwrap();

        // PATCH — update title
        let update_body = serde_json::json!({ "title": "Updated Title" });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-1")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "update discussion: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify title changed
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-1")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["title"].as_str().unwrap(), "Updated Title");

        // PATCH — archive
        let archive_body = serde_json::json!({ "archived": true });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-1")
            .header("Content-Type", "application/json")
            .body(Body::from(archive_body.to_string()))
            .unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify archived
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-1")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert!(body["data"]["archived"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn discussions_create_with_profile_and_directive_ids() {
        let state = test_state();

        // Create discussion with profile_ids and directive_ids
        let create_body = serde_json::json!({
            "title": "Discussion with extras",
            "agent": "ClaudeCode",
            "language": "en",
            "initial_prompt": "Hello with profiles",
            "profile_ids": ["profile-dev", "profile-reviewer"],
            "directive_ids": ["directive-eco", "directive-security"]
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "create discussion with profiles/directives"
        );

        // Wait for background persistence
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // List and verify stored profile_ids / directive_ids
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discussions = body["data"].as_array().unwrap();
        assert_eq!(discussions.len(), 1);
        let disc = &discussions[0];
        let profile_ids = disc["profile_ids"].as_array().unwrap();
        let directive_ids = disc["directive_ids"].as_array().unwrap();
        assert_eq!(profile_ids.len(), 2, "Should store 2 profile_ids");
        assert_eq!(directive_ids.len(), 2, "Should store 2 directive_ids");
        assert!(profile_ids
            .iter()
            .any(|v| v.as_str() == Some("profile-dev")));
        assert!(directive_ids
            .iter()
            .any(|v| v.as_str() == Some("directive-eco")));
    }

    #[tokio::test]
    async fn discussions_patch_title() {
        let state = test_state();
        insert_test_discussion(&state, "disc-patch-title", "Old Title").await;

        let update_body = serde_json::json!({ "title": "New Title" });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-patch-title")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "PATCH title: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify title changed
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-patch-title")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["title"].as_str().unwrap(), "New Title");
    }

    #[tokio::test]
    async fn discussions_delete() {
        let state = test_state();

        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let disc = crate::models::Discussion {
                    awaiting_agent: false,
                    agent_running: false,
                    id: "disc-del".into(),
                    project_id: None,
                    title: "To Delete".into(),
                    agent: crate::models::AgentType::Vibe,
                    language: "fr".into(),
                    participants: vec![],
                    message_count: 0,
                    non_system_message_count: 0,
                    messages: vec![],
                    skill_ids: vec![],
                    profile_ids: vec![],
                    directive_ids: vec![],
                    archived: false,
                    pinned: false,
                    workspace_mode: "Direct".into(),
                    workspace_path: None,
                    worktree_branch: None,
                    tier: crate::models::ModelTier::Default,
                    model: None,
                    pin_first_message: false,
                    summary_cache: None,
                    summary_up_to_msg_idx: None,
                    summary_strategy: crate::models::SummaryStrategy::Auto,
                    introspection_call_count: 0,
                    shared_id: None,
                    shared_with: vec![],
                    workflow_run_id: None,
                    test_mode_restore_branch: None,
                    test_mode_stash_ref: None,
                    created_at: now,
                    updated_at: now,
                };
                crate::db::discussions::insert_discussion(conn, &disc)?;
                Ok(())
            })
            .await
            .unwrap();

        // DELETE
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/discussions/disc-del")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // Verify gone
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-del")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body["success"].as_bool().unwrap_or(true),
            "Deleted discussion should return error"
        );
    }

    #[tokio::test]
    async fn discussions_create_validates_title_length() {
        let state = test_state();

        let long_title = "x".repeat(501);
        let create_body = serde_json::json!({
            "title": long_title,
            "agent": "ClaudeCode",
            "initial_prompt": "test"
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/discussions")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        // Should reject with validation error
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body["success"].as_bool().unwrap_or(true),
            "Title >500 chars should be rejected: {body}"
        );
    }

    #[tokio::test]
    async fn source_session_binding_is_versioned_visible_and_not_stolen_silently() {
        let state = test_state();
        insert_test_discussion(&state, "disc-source-a", "Source A").await;
        insert_test_discussion(&state, "disc-source-b", "Source B").await;

        let link = |disc_id: &str, force_reassign: bool| {
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "disc_id": disc_id,
                        "source_agent": "Codex",
                        "source_session_id": "codex-session-42",
                        "force_reassign": force_reassign,
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let (status, body) = send(state.clone(), false, link("disc-source-a", false)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "{body}");

        let status_request = || {
            Request::builder()
                .uri(
                    "/api/disc/session-status?source_agent=Codex&source_session_id=codex-session-42",
                )
                .body(Body::empty())
                .unwrap()
        };
        let (_, body) = send(state.clone(), false, status_request()).await;
        assert_eq!(body["data"]["binding_version"], 1);
        assert_eq!(body["data"]["bound_disc_id"], "disc-source-a");
        assert!(body["data"]["connected_disc_id"].is_null());

        let (_, conflict) = send(state.clone(), false, link("disc-source-b", false)).await;
        assert_eq!(conflict["success"], false);
        assert!(conflict["error"]
            .as_str()
            .unwrap()
            .contains("already linked to discussion disc-source-a"));

        state
            .db
            .with_conn(|connection| {
                crate::db::discussion_sessions::create_session(
                    connection,
                    "disc-source-a",
                    "Codex",
                    Some("codex-session-42"),
                    "peer",
                )
            })
            .await
            .unwrap();
        let (_, connected) = send(state.clone(), false, status_request()).await;
        assert_eq!(connected["data"]["connected_disc_id"], "disc-source-a");
        assert_eq!(connected["data"]["connection_status"], "active");

        let (_, transferred) = send(state.clone(), false, link("disc-source-b", true)).await;
        assert_eq!(transferred["success"], true, "{transferred}");
        let (_, body) = send(state, false, status_request()).await;
        assert_eq!(body["data"]["bound_disc_id"], "disc-source-b");
        assert_eq!(
            body["data"]["connected_disc_id"], "disc-source-a",
            "source ownership may transfer while the old CLI peer is still live"
        );
    }

    #[tokio::test]
    async fn source_session_transfer_is_explicit_audited_and_idempotent() {
        let state = test_state();
        insert_test_discussion(&state, "disc-transfer-a", "Transfer A").await;
        insert_test_discussion(&state, "disc-transfer-b", "Transfer B").await;
        insert_test_discussion(&state, "disc-transfer-c", "Transfer C").await;

        let link = Request::builder()
            .method("POST")
            .uri("/api/disc/link")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "disc_id": "disc-transfer-a",
                    "source_agent": "Codex",
                    "source_session_id": "codex-transfer-session",
                })
                .to_string(),
            ))
            .unwrap();
        let (_, linked) = send(state.clone(), false, link).await;
        assert_eq!(linked["success"], true, "{linked}");

        let transfer = |from_disc_id: &str, confirm_transfer: bool| {
            Request::builder()
                .method("POST")
                .uri("/api/disc/transfer-session")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "from_disc_id": from_disc_id,
                        "to_disc_id": "disc-transfer-b",
                        "source_agent": "Codex",
                        "source_session_id": "codex-transfer-session",
                        "confirm_transfer": confirm_transfer,
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        let (_, unconfirmed) = send(state.clone(), false, transfer("disc-transfer-a", false)).await;
        assert_eq!(unconfirmed["success"], false);
        assert!(unconfirmed["error"]
            .as_str()
            .unwrap()
            .contains("confirm_transfer=true"));

        let (_, stale_owner) = send(state.clone(), false, transfer("disc-transfer-c", true)).await;
        assert_eq!(stale_owner["success"], false);
        assert!(stale_owner["error"]
            .as_str()
            .unwrap()
            .contains("ownership changed"));

        let (_, transferred) = send(state.clone(), false, transfer("disc-transfer-a", true)).await;
        assert_eq!(transferred["success"], true, "{transferred}");
        assert_eq!(transferred["data"]["previous_disc_id"], "disc-transfer-a");
        assert_eq!(transferred["data"]["disc_id"], "disc-transfer-b");
        assert_eq!(transferred["data"]["session_bound"], true);
        assert_eq!(transferred["data"]["transferred"], true);
        assert_eq!(transferred["data"]["binding_version"], 1);

        let (_, replayed) = send(state.clone(), false, transfer("disc-transfer-a", true)).await;
        assert_eq!(replayed["success"], true, "{replayed}");
        assert_eq!(replayed["data"]["session_bound"], true);
        assert_eq!(replayed["data"]["transferred"], false);

        let (_, wrong_replay) = send(state.clone(), false, transfer("disc-transfer-c", true)).await;
        assert_eq!(wrong_replay["success"], false);
        assert!(wrong_replay["error"]
            .as_str()
            .unwrap()
            .contains("no completed transfer"));

        let find = Request::builder()
            .uri(
                "/api/disc/find_by_session?source_agent=Codex&source_session_id=codex-transfer-session",
            )
            .body(Body::empty())
            .unwrap();
        let (_, resumed) = send(state.clone(), false, find).await;
        assert_eq!(resumed["data"]["disc_id"], "disc-transfer-b");

        let (old_history, new_history) = state
            .db
            .with_read_conn(|connection| {
                Ok((
                    crate::db::disc_source::list_source_history(connection, "disc-transfer-a")?,
                    crate::db::disc_source::list_source_history(connection, "disc-transfer-b")?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(old_history.len(), 1);
        assert!(
            old_history[0].unlinked_at.is_some(),
            "the previous binding remains in the audit chain as closed"
        );
        assert_eq!(new_history.len(), 1);
        assert!(
            new_history[0].unlinked_at.is_none(),
            "the target binding is the only open owner"
        );
    }

    #[tokio::test]
    async fn discussion_export_import_is_versioned_idempotent_and_conflict_aware() {
        let state = test_state();
        insert_test_discussion(&state, "disc-portability-http", "Portable HTTP").await;
        state
            .db
            .with_conn(|connection| {
                crate::db::discussions::insert_message(
                    connection,
                    "disc-portability-http",
                    &crate::models::DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        id: "portable-http-message".into(),
                        role: crate::models::MessageRole::User,
                        channel: crate::models::MessageChannel::Main,
                        content: "Export me".into(),
                        agent_type: None,
                        timestamp: chrono::Utc::now(),
                        tokens_used: 0,
                        auth_mode: None,
                        model_tier: None,
                        model: None,
                        cost_usd: None,
                        author_pseudo: Some("Tester".into()),
                        author_avatar_email: None,
                        source_msg_id: None,
                        duration_ms: None,
                        lint_report: None,
                        target_agent: None,
                        reply_to_message_id: None,
                    },
                )?;
                Ok(())
            })
            .await
            .unwrap();

        let export_request = || {
            Request::builder()
                .uri("/api/discussions/disc-portability-http/export")
                .body(Body::empty())
                .unwrap()
        };
        let (status, bundle) = send(state.clone(), false, export_request()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bundle["kind"], "kronn.discussion");
        assert_eq!(bundle["version"], 1);
        assert_eq!(bundle["messages"].as_array().unwrap().len(), 1);

        let import_request = |content: &Value| {
            Request::builder()
                .method("POST")
                .uri("/api/discussions/import")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "content": content.to_string(),
                        "project_id": null,
                    })
                    .to_string(),
                ))
                .unwrap()
        };
        let (_, imported) = send(state.clone(), false, import_request(&bundle)).await;
        assert_eq!(imported["success"], true, "{imported}");
        assert_eq!(imported["data"]["already_imported"], false);
        assert_eq!(imported["data"]["imported_messages"], 1);
        let imported_id = imported["data"]["discussion_id"]
            .as_str()
            .unwrap()
            .to_string();

        let (_, replay) = send(state.clone(), false, import_request(&bundle)).await;
        assert_eq!(replay["success"], true, "{replay}");
        assert_eq!(replay["data"]["already_imported"], true);
        assert_eq!(replay["data"]["discussion_id"], imported_id);

        let mut changed = bundle;
        changed["discussion"]["title"] = Value::String("Changed".into());
        let (_, conflict) = send(state, false, import_request(&changed)).await;
        assert_eq!(conflict["success"], false);
        assert_eq!(conflict["error_code"], "conflict");
    }

    #[tokio::test]
    async fn guided_tour_demo_is_idempotent_agentless_and_reopened() {
        let state = test_state();
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/api/tour/demo-discussion")
                .body(Body::empty())
                .unwrap()
        };

        let (status, first) = send(state.clone(), false, request()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["success"], true, "{first}");
        assert_eq!(first["data"]["created"], true);
        assert_eq!(
            first["data"]["prompt"],
            "Crée une courte page HTML présentant Kronn dans le viewer de documents."
        );
        let discussion_id = first["data"]["discussion_id"]
            .as_str()
            .expect("tour demo id")
            .to_string();

        let archived_id = discussion_id.clone();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "UPDATE discussions SET archived = 1 WHERE id = ?1",
                    [&archived_id],
                )?;
                Ok(())
            })
            .await
            .unwrap();

        state.config.write().await.ui_language = "es".into();
        let (_, replay) = send(state.clone(), false, request()).await;
        assert_eq!(replay["success"], true, "{replay}");
        assert_eq!(replay["data"]["created"], false);
        assert_eq!(replay["data"]["discussion_id"], discussion_id);
        assert_eq!(
            replay["data"]["prompt"],
            "Crea una breve página HTML sobre Kronn en el visor de documentos."
        );

        let inspected_id = discussion_id.clone();
        let (
            discussion_count,
            message_count,
            reply_count,
            import_count,
            provenance_kind,
            no_agent,
            archived,
            language,
            preview_content,
        ) = state
            .db
            .with_conn(move |connection| {
                let discussion_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM discussions WHERE id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let message_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM messages WHERE discussion_id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let reply_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM messages
                         WHERE discussion_id = ?1
                           AND reply_to_message_id IS NOT NULL
                           AND content LIKE '%kronn-doc-preview%'",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let import_count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM discussion_imports
                         WHERE imported_discussion_id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let provenance_kind: String = connection.query_row(
                    "SELECT provenance_kind FROM discussion_imports
                         WHERE imported_discussion_id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let no_agent: i64 = connection.query_row(
                    "SELECT no_agent FROM discussions WHERE id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let archived: i64 = connection.query_row(
                    "SELECT archived FROM discussions WHERE id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let language: String = connection.query_row(
                    "SELECT language FROM discussions WHERE id = ?1",
                    [&inspected_id],
                    |row| row.get(0),
                )?;
                let preview_content: String = connection.query_row(
                    "SELECT content FROM messages
                     WHERE discussion_id = ?1 AND source_msg_id = ?2",
                    [&inspected_id, "kronn-guided-tour-demo-preview"],
                    |row| row.get(0),
                )?;
                Ok((
                    discussion_count,
                    message_count,
                    reply_count,
                    import_count,
                    provenance_kind,
                    no_agent,
                    archived,
                    language,
                    preview_content,
                ))
            })
            .await
            .unwrap();

        assert_eq!(discussion_count, 1);
        assert_eq!(message_count, 2);
        assert_eq!(reply_count, 1);
        assert_eq!(import_count, 1);
        assert_eq!(provenance_kind, "guided_tour_demo");
        assert_eq!(no_agent, 1);
        assert_eq!(archived, 0, "replaying the tour reopens its demo");
        assert_eq!(language, "es");
        assert!(preview_content.contains("Vista previa del documento"));
        assert!(preview_content.contains("<html lang=\"es\">"));
    }

    #[tokio::test]
    async fn plugin_bundle_http_round_trip_requires_passphrase_for_values() {
        use std::collections::HashMap;

        let source = test_state();
        let source_secret = source
            .config
            .read()
            .await
            .encryption_secret
            .clone()
            .unwrap();
        source
            .db
            .with_conn(move |connection| {
                let server = crate::models::McpServer {
                    id: "custom-http-portable".into(),
                    name: "HTTP Portable".into(),
                    description: "HTTP test".into(),
                    transport: crate::models::McpTransport::ApiOnly,
                    source: crate::models::McpSource::Manual,
                    api_spec: Some(crate::models::ApiSpec {
                        base_url: "https://example.test".into(),
                        auth: crate::models::ApiAuthKind::Bearer {
                            env_key: "API_TOKEN".into(),
                        },
                        endpoints: vec![],
                        docs_url: None,
                        config_keys: vec![],
                    }),
                };
                crate::db::mcps::upsert_server(connection, &server)?;
                let env = HashMap::from([("API_TOKEN".into(), "portable-secret".into())]);
                let encrypted = crate::db::mcps::encrypt_env(&env, &source_secret)
                    .map_err(anyhow::Error::msg)?;
                crate::db::mcps::insert_config(
                    connection,
                    &crate::models::McpConfig {
                        id: "config-http-portable".into(),
                        server_id: server.id.clone(),
                        label: "HTTP Portable".into(),
                        env_keys: vec!["API_TOKEN".into()],
                        env_encrypted: encrypted,
                        args_override: None,
                        is_global: false,
                        include_general: true,
                        config_hash: crate::db::mcps::compute_config_hash(&server, &env, None),
                        project_ids: vec![],
                        host_sync: crate::models::HostSyncMode::None,
                    },
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let preview = Request::builder()
            .method("POST")
            .uri("/api/mcps/bundles/preview")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"config_ids": ["config-http-portable"]}).to_string(),
            ))
            .unwrap();
        let (_, preview_body) = send(source.clone(), false, preview).await;
        assert_eq!(preview_body["success"], true, "{preview_body}");
        assert_eq!(preview_body["data"]["sensitive_value_count"], 1);

        let export = Request::builder()
            .method("POST")
            .uri("/api/mcps/bundles/export")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "config_ids": ["config-http-portable"],
                    "include_values": true,
                    "passphrase": "portable passphrase",
                    "confirmation": "EXPORTER LES SECRETS"
                })
                .to_string(),
            ))
            .unwrap();
        let (status, bundle) = send(source, false, export).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(bundle["kind"], "kronn.plugins");
        assert_eq!(bundle["encrypted"], true);
        assert!(bundle.get("payload").is_none());
        assert!(
            !bundle.to_string().contains("portable-secret"),
            "encrypted bundle must not leak a value"
        );

        let target = test_state();
        let import_request = |passphrase: Option<&str>| {
            Request::builder()
                .method("POST")
                .uri("/api/mcps/bundles/import")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "content": bundle.to_string(),
                        "passphrase": passphrase,
                    })
                    .to_string(),
                ))
                .unwrap()
        };
        let (_, refused) = send(target.clone(), false, import_request(None)).await;
        assert_eq!(refused["success"], false);
        assert_eq!(refused["error_code"], "validation");

        let (_, imported) = send(
            target.clone(),
            false,
            import_request(Some("portable passphrase")),
        )
        .await;
        assert_eq!(imported["success"], true, "{imported}");
        assert_eq!(
            imported["data"]["imported_config_ids"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let (_, replay) = send(target, false, import_request(Some("portable passphrase"))).await;
        assert_eq!(replay["success"], true, "{replay}");
        assert_eq!(replay["data"]["already_imported"], true);
    }

    // ─── Q9: Agents API integration tests ─────────────────────────────────────

    #[tokio::test]
    async fn agents_detect_returns_list() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/agents")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let agents = body["data"].as_array().unwrap();
        // Should detect at least some agents (even if not installed)
        assert!(
            !agents.is_empty(),
            "Agent detection should return at least one entry"
        );
        // Each agent should have required fields
        for agent in agents {
            assert!(agent["name"].is_string(), "Agent should have name");
            assert!(
                agent["agent_type"].is_string(),
                "Agent should have agent_type"
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn agents_toggle_changes_state() {
        isolate_config_dir();
        let state = test_state();

        // Toggle Vibe off
        let req = Request::builder()
            .method("POST")
            .uri("/api/agents/toggle")
            .header("Content-Type", "application/json")
            .body(Body::from("\"Vibe\""))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "toggle agent: {body}");
        assert!(body["success"].as_bool().unwrap());
        let enabled = body["data"].as_bool().unwrap();

        // Toggle again — should flip
        let req = Request::builder()
            .method("POST")
            .uri("/api/agents/toggle")
            .header("Content-Type", "application/json")
            .body(Body::from("\"Vibe\""))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let new_enabled = body["data"].as_bool().unwrap();
        assert_ne!(enabled, new_enabled, "Toggle should flip the enabled state");
    }

    // ─── Q10: Skills API integration tests ────────────────────────────────────

    #[tokio::test]
    async fn skills_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/skills")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let skills = body["data"].as_array().unwrap();
        assert!(!skills.is_empty(), "Should have built-in skills");
        // Verify structure
        let first = &skills[0];
        assert!(first["id"].is_string());
        assert!(first["name"].is_string());
    }

    // ─── Q11: Profiles API integration tests ──────────────────────────────────

    #[tokio::test]
    async fn profiles_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/profiles")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let profiles = body["data"].as_array().unwrap();
        assert!(!profiles.is_empty(), "Should have built-in profiles");
    }

    // ─── Q12: Directives API integration tests ───────────────────────────────

    #[tokio::test]
    async fn directives_list_returns_builtins() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/directives")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        let directives = body["data"].as_array().unwrap();
        assert!(!directives.is_empty(), "Should have built-in directives");
    }

    // ─── Q13: Config API additional tests ─────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn config_server_get_and_set() {
        isolate_config_dir();
        let state = test_state();

        // GET current server config
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/server")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());

        // POST new server config — enable auth
        let new_config = serde_json::json!({
            "auth_enabled": true,
            "auth_token": null,
            "max_concurrent_agents": 3
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/config/server")
            .header("Content-Type", "application/json")
            .body(Body::from(new_config.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "set server config: {body}");
    }

    #[tokio::test]
    #[serial]
    async fn config_agent_mention_color_validates_and_persists() {
        isolate_config_dir();
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/config/agent-mention-color")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "agent": "Codex", "color": "#A1b2C3" }).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "{body}");
        assert_eq!(
            state
                .config
                .read()
                .await
                .agents
                .codex
                .mention_color
                .as_deref(),
            Some("#a1b2c3")
        );

        let persisted = crate::core::config::load()
            .await
            .expect("load persisted config")
            .expect("saved config exists");
        assert_eq!(
            persisted.agents.codex.mention_color.as_deref(),
            Some("#a1b2c3")
        );
    }

    #[tokio::test]
    #[serial]
    async fn config_agent_mention_color_rejects_invalid_css_values() {
        isolate_config_dir();
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/config/agent-mention-color")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({ "agent": "Codex", "color": "red" }).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false, "{body}");
        assert!(state
            .config
            .read()
            .await
            .agents
            .codex
            .mention_color
            .is_none());
    }

    #[tokio::test]
    async fn config_scan_paths_get() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/scan-paths")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn config_tokens_get() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/tokens")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn config_db_info() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/db-info")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── Q14: Discussion message operations ───────────────────────────────────

    /// Helper: insert a discussion directly in DB for fast test setup
    async fn insert_test_discussion(state: &AppState, id: &str, title: &str) {
        state
            .db
            .with_conn({
                let id = id.to_string();
                let title = title.to_string();
                move |conn| {
                    let now = chrono::Utc::now();
                    let disc = crate::models::Discussion {
                        awaiting_agent: false,
                        agent_running: false,
                        id: id.clone(),
                        project_id: None,
                        title,
                        agent: crate::models::AgentType::ClaudeCode,
                        language: "en".into(),
                        participants: vec![crate::models::AgentType::ClaudeCode],
                        message_count: 0,
                        non_system_message_count: 0,
                        messages: vec![],
                        skill_ids: vec![],
                        profile_ids: vec![],
                        directive_ids: vec![],
                        archived: false,
                        pinned: false,
                        workspace_mode: "Direct".into(),
                        workspace_path: None,
                        worktree_branch: None,
                        tier: crate::models::ModelTier::Default,
                        model: None,
                        pin_first_message: false,
                        summary_cache: None,
                        summary_up_to_msg_idx: None,
                        summary_strategy: crate::models::SummaryStrategy::Auto,
                        introspection_call_count: 0,
                        shared_id: None,
                        shared_with: vec![],
                        workflow_run_id: None,
                        test_mode_restore_branch: None,
                        test_mode_stash_ref: None,
                        created_at: now,
                        updated_at: now,
                    };
                    crate::db::discussions::insert_discussion(conn, &disc)?;
                    Ok(())
                }
            })
            .await
            .unwrap();
    }

    /// Helper: insert a message directly in DB
    async fn insert_test_message(state: &AppState, disc_id: &str, role: &str, content: &str) {
        state
            .db
            .with_conn({
                let disc_id = disc_id.to_string();
                let role = role.to_string();
                let content = content.to_string();
                move |conn| {
                    let msg = crate::models::DiscussionMessage {
                        recovered_partial: false,
                        session_tokens_at_message: None,
                        author_cli_ordinal: None,
                        model: None,
                        lint_report: None,
                        id: uuid::Uuid::new_v4().to_string(),
                        role: match role.as_str() {
                            "User" => crate::models::MessageRole::User,
                            "Agent" => crate::models::MessageRole::Agent,
                            _ => crate::models::MessageRole::System,
                        },
                        channel: crate::models::MessageChannel::Main,
                        content,
                        agent_type: if role == "Agent" {
                            Some(crate::models::AgentType::ClaudeCode)
                        } else {
                            None
                        },
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
                    crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
                    Ok(())
                }
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn discussions_delete_last_agent_messages() {
        let state = test_state();
        insert_test_discussion(&state, "disc-msg", "Message Test").await;
        insert_test_message(&state, "disc-msg", "User", "Hello").await;
        insert_test_message(&state, "disc-msg", "Agent", "Agent reply").await;
        insert_test_message(&state, "disc-msg", "Agent", "Agent follow up").await;

        // DELETE last agent messages
        let req = Request::builder()
            .method("DELETE")
            .uri("/api/discussions/disc-msg/messages/last")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "delete last agent messages: {body}");
        assert!(body["success"].as_bool().unwrap());

        // Verify: discussion should only have the user message now
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-msg")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        let messages = body["data"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "Only user message should remain");
        assert_eq!(messages[0]["role"].as_str().unwrap(), "User");
    }

    #[tokio::test]
    async fn discussions_edit_last_user_message() {
        let state = test_state();
        insert_test_discussion(&state, "disc-edit", "Edit Test").await;
        insert_test_message(&state, "disc-edit", "User", "Original message").await;
        insert_test_message(&state, "disc-edit", "Agent", "Agent reply").await;

        // PATCH last user message
        let edit_body = serde_json::json!({ "content": "Edited message" });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-edit/messages/last")
            .header("Content-Type", "application/json")
            .body(Body::from(edit_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "edit last user message: {body}");

        // Verify: user message content updated, agent messages removed
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-edit")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        let messages = body["data"]["messages"].as_array().unwrap();
        // After edit, agent messages should be deleted and user message updated
        let user_msgs: Vec<_> = messages
            .iter()
            .filter(|m| m["role"].as_str() == Some("User"))
            .collect();
        assert!(
            !user_msgs.is_empty(),
            "Should have at least one user message"
        );
        assert_eq!(
            user_msgs.last().unwrap()["content"].as_str().unwrap(),
            "Edited message"
        );
    }

    #[tokio::test]
    async fn discussions_update_skill_ids() {
        let state = test_state();
        insert_test_discussion(&state, "disc-skills", "Skills Test").await;

        let update_body = serde_json::json!({
            "skill_ids": ["skill-rust", "skill-testing"]
        });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-skills")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-skills")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        let skill_ids = body["data"]["skill_ids"].as_array().unwrap();
        assert_eq!(skill_ids.len(), 2);
    }

    #[tokio::test]
    async fn discussions_update_tier() {
        let state = test_state();
        insert_test_discussion(&state, "disc-tier", "Tier Test").await;

        let update_body = serde_json::json!({ "tier": "economy" });
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/discussions/disc-tier")
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-tier")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["data"]["tier"].as_str().unwrap(), "economy");
    }

    // ─── Q15: Workflow CRUD API tests ─────────────────────────────────────────

    #[tokio::test]
    async fn workflows_list_empty() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/workflows")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn workflows_create_and_get() {
        let state = test_state();

        let create_body = serde_json::json!({
            "name": "Nightly Audit",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "audit",
                "agent": "ClaudeCode",
                "prompt_template": "Run audit on project",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": {
                "sandbox": false,
                "max_files": null,
                "max_lines": null,
                "require_approval": false
            }
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "create workflow: {body}");
        let wf_id = body["data"]["id"].as_str().unwrap().to_string();
        let created_step_id = body["data"]["steps"][0]["id"]
            .as_str()
            .expect("created workflow step must have a durable id")
            .to_string();
        assert!(uuid::Uuid::parse_str(&created_step_id).is_ok());

        // GET by ID
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["name"].as_str().unwrap(), "Nightly Audit");
        assert_eq!(body["data"]["steps"].as_array().unwrap().len(), 1);
        assert_eq!(
            body["data"]["steps"][0]["id"].as_str(),
            Some(created_step_id.as_str()),
            "the persisted step id must remain stable across reads"
        );
    }

    /// 0.8.5 — `workflow_create_draft` MCP tool round-trip.
    ///
    /// Critical safety contract: when the MCP tool POSTs with
    /// `enabled: false`, the persisted workflow MUST stay disabled.
    /// Without this, an agent draft would fire on its cron schedule
    /// before the user has reviewed it — exactly the failure mode the
    /// draft path was designed to prevent.
    #[tokio::test]
    async fn create_workflow_with_enabled_false_persists_as_draft() {
        let state = test_state();
        let create_body = serde_json::json!({
            "name": "Draft from MCP agent",
            "trigger": { "type": "Cron", "schedule": "0 9 * * 1-5" },
            "steps": [{
                "name": "s1",
                "agent": "ClaudeCode",
                "prompt_template": "review the staging logs",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "enabled": false,
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "draft create: {body}");
        assert_eq!(
            body["data"]["enabled"].as_bool(),
            Some(false),
            "draft workflow MUST persist with enabled=false (no auto-fire)"
        );
        // Round-trip GET to make sure the value didn't flip on the way
        // through the DB serialiser.
        let wf_id = body["data"]["id"].as_str().unwrap().to_string();
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(
            body["data"]["enabled"].as_bool(),
            Some(false),
            "draft persists as disabled across read"
        );
    }

    /// 0.8.5 — back-compat: every UI-driven POST without `enabled`
    /// must continue to land as `enabled: true` (the default Workflow
    /// state since 0.5.x). The optional field can't accidentally
    /// disable existing user flows.
    #[tokio::test]
    async fn create_workflow_without_enabled_field_defaults_to_true() {
        let state = test_state();
        let create_body = serde_json::json!({
            "name": "UI Create",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "s1",
                "agent": "ClaudeCode",
                "prompt_template": "do the thing",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(
            body["data"]["enabled"].as_bool(),
            Some(true),
            "default behaviour MUST stay enabled=true when the field is omitted (back-compat)"
        );
    }

    #[tokio::test]
    async fn workflows_update_and_delete() {
        let state = test_state();

        // Create
        let create_body = serde_json::json!({
            "name": "To Update",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "s1",
                "agent": "Vibe",
                "prompt_template": "test",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": { "sandbox": false, "max_files": null, "max_lines": null, "require_approval": false }
        });

        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows")
            .header("Content-Type", "application/json")
            .body(Body::from(create_body.to_string()))
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let wf_id = body["data"]["id"].as_str().unwrap().to_string();

        // Update
        let update_body = serde_json::json!({
            "name": "Updated Name",
            "trigger": { "type": "Manual" },
            "steps": [{
                "name": "s1-updated",
                "agent": "ClaudeCode",
                "prompt_template": "updated prompt",
                "mode": { "type": "Normal" }
            }],
            "actions": [],
            "safety": { "sandbox": false, "max_files": null, "max_lines": null, "require_approval": false }
        });

        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/workflows/{}", wf_id))
            .header("Content-Type", "application/json")
            .body(Body::from(update_body.to_string()))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "update workflow: {body}");

        // Verify updated
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(body["data"]["name"].as_str().unwrap(), "Updated Name");

        // Delete
        let req = Request::builder()
            .method("DELETE")
            .uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);

        // Verify gone
        let req = Request::builder()
            .method("GET")
            .uri("/api/workflows")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 0);
    }

    // ─── Q15b: Config model-tiers API ─────────────────────────────────────────

    #[tokio::test]
    async fn config_model_tiers_returns_config() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/model-tiers")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // model-tiers should return an object with tier configuration
        assert!(
            body["data"].is_object(),
            "model-tiers should return a config object"
        );
    }

    // ─── Q16: Export/Import API ───────────────────────────────────────────────

    #[tokio::test]
    async fn config_export_returns_zip() {
        let state = test_state();
        let app = build_router_with_auth(state, false);
        let req = Request::builder()
            .method("GET")
            .uri("/api/config/export")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.expect("oneshot failed");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "application/zip"
        );
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        // ZIP magic bytes
        assert!(bytes.len() > 4);
        assert_eq!(bytes[0], b'P');
        assert_eq!(bytes[1], b'K');
    }

    // ─── Q17: Agent usage stats ───────────────────────────────────────────────

    #[tokio::test]
    async fn stats_agent_usage_returns_ok() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/stats/agent-usage")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
    }

    // ─── MCP overview includes incompatibilities ──────────────────────────────

    #[tokio::test]
    async fn mcp_overview_includes_incompatibilities_field() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/mcps")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap());
        // incompatibilities field should exist (may be empty if no gitlab server in test DB)
        assert!(
            body["data"]["incompatibilities"].is_array(),
            "McpOverview must include incompatibilities array"
        );
    }

    // ─── Error hint detection ────────────────────────────────────────────────

    #[tokio::test]
    async fn detect_error_hint_mcp_config() {
        use crate::api::discussions::detect_agent_error_hint;
        use crate::models::AgentType;
        let hint = detect_agent_error_hint(
            "Error: Invalid MCP configuration: MCP config file not found: /host-home/Repositories/test/",
            &AgentType::ClaudeCode,
        );
        assert!(hint.is_some(), "Should detect MCP config error");
        assert!(hint.unwrap().contains("MCP"), "Hint should mention MCP");
    }

    #[tokio::test]
    async fn detect_error_hint_auth() {
        use crate::api::discussions::detect_agent_error_hint;
        use crate::models::AgentType;
        let hint = detect_agent_error_hint(
            "authentication_error: invalid API key",
            &AgentType::ClaudeCode,
        );
        assert!(hint.is_some(), "Should detect auth error");
    }

    #[tokio::test]
    async fn detect_error_hint_no_match() {
        use crate::api::discussions::detect_agent_error_hint;
        use crate::models::AgentType;
        let hint =
            detect_agent_error_hint("Everything is fine, no errors here", &AgentType::ClaudeCode);
        assert!(hint.is_none(), "Should not detect error in normal output");
    }

    // ─── Drift detection API tests ──────────────────────────────────────────

    #[tokio::test]
    async fn drift_check_no_project() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/nonexistent/drift")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK); // API returns 200 with success=false
        assert!(
            !body["success"].as_bool().unwrap_or(true),
            "Drift check on nonexistent project should return error: {body}"
        );
    }

    #[tokio::test]
    async fn drift_check_route_exists() {
        let state = test_state();

        // Insert a project with a real path so check_drift can run
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "drift-proj".into(),
                    name: "Drift Test Project".into(),
                    path: "/tmp/kronn-drift-test".into(),
                    repo_url: None,
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        // Ensure the path exists (even if empty)
        std::fs::create_dir_all("/tmp/kronn-drift-test").ok();

        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/drift-proj/drift")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "drift check route should return 200: {body}"
        );
        assert!(
            body["success"].as_bool().unwrap_or(false),
            "drift check should succeed (empty drift): {body}"
        );
    }

    #[tokio::test]
    async fn partial_audit_invalid_steps() {
        let state = test_state();

        // Insert a project
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "partial-proj".into(),
                    name: "Partial Audit Test".into(),
                    path: "/tmp/kronn-partial-test".into(),
                    repo_url: None,
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        std::fs::create_dir_all("/tmp/kronn-partial-test").ok();

        // POST with invalid step number (99)
        let body_json = serde_json::json!({
            "agent": "ClaudeCode",
            "steps": [99]
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/partial-proj/partial-audit")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let app = build_router_with_auth(state, false);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        assert_eq!(resp.status(), StatusCode::OK, "SSE endpoint returns 200");

        // Consume SSE body and check for error event about invalid step
        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body_str = String::from_utf8_lossy(&bytes);
        assert!(
            body_str.contains("Invalid step"),
            "Should contain error about invalid step: {body_str}"
        );
    }

    #[tokio::test]
    async fn partial_audit_route_exists() {
        let state = test_state();

        // Insert a project
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "partial-ok-proj".into(),
                    name: "Partial OK Test".into(),
                    path: "/tmp/kronn-partial-ok-test".into(),
                    repo_url: None,
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        std::fs::create_dir_all("/tmp/kronn-partial-ok-test").ok();

        // POST with valid step number (1)
        let body_json = serde_json::json!({
            "agent": "ClaudeCode",
            "steps": [1]
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/partial-ok-proj/partial-audit")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let app = build_router_with_auth(state, false);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        // SSE always returns 200
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "partial-audit route should return 200 (SSE)"
        );
    }

    #[tokio::test]
    async fn briefing_get_set() {
        let state = test_state();

        // Create a project
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "briefing-proj".into(),
                    name: "Briefing Test".into(),
                    path: "/tmp/briefing-test".into(),
                    repo_url: None,
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        // GET briefing — should be null initially
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false));
        assert!(
            body["data"].is_null(),
            "Briefing should be null initially: {body}"
        );

        // PUT briefing — set notes
        let req = Request::builder()
            .method("PUT")
            .uri("/api/projects/briefing-proj/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(
                r#"{"notes":"This is a Node.js monorepo with React frontend"}"#,
            ))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["success"].as_bool().unwrap_or(false),
            "Set briefing should succeed: {body}"
        );

        // GET briefing — should return the notes
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"].as_str().unwrap(),
            "This is a Node.js monorepo with React frontend"
        );

        // PUT briefing — clear notes
        let req = Request::builder()
            .method("PUT")
            .uri("/api/projects/briefing-proj/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"notes":null}"#))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["success"].as_bool().unwrap_or(false));

        // GET briefing — should be null again
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/briefing-proj/briefing")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["data"].is_null(),
            "Briefing should be null after clearing: {body}"
        );
    }

    #[tokio::test]
    async fn briefing_nonexistent_project() {
        let state = test_state();

        // GET briefing for nonexistent project — should return null (no project row found)
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects/nonexistent/briefing")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["data"].is_null(),
            "Briefing for nonexistent project should be null"
        );

        // PUT briefing for nonexistent project — should fail
        let req = Request::builder()
            .method("PUT")
            .uri("/api/projects/nonexistent/briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(r#"{"notes":"test"}"#))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            !body["success"].as_bool().unwrap_or(true),
            "Set briefing on nonexistent project should fail: {body}"
        );
    }

    // ─── Start briefing tests ────────────────────────────────────────────

    #[tokio::test]
    async fn start_briefing_route_exists() {
        let state = test_state();

        // Create a project in DB
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let project = crate::models::Project {
                    id: "briefing-start-proj".into(),
                    name: "Start Briefing Test".into(),
                    path: "/tmp/kronn-start-briefing-test".into(),
                    repo_url: None,
                    token_override: None,
                    ai_config: crate::models::AiConfigStatus {
                        detected: false,
                        configs: vec![],
                    },
                    audit_status: crate::models::AiAuditStatus::NoTemplate,
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
                crate::db::projects::insert_project(conn, &project)?;
                Ok(())
            })
            .await
            .unwrap();

        let body_json = serde_json::json!({ "agent": "ClaudeCode" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/briefing-start-proj/start-briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let (status, body) = send(state, false, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "start-briefing should return 200: {body}"
        );
        assert!(
            body["success"].as_bool().unwrap_or(false),
            "start-briefing should succeed: {body}"
        );
        assert!(
            body["data"]["discussion_id"].is_string(),
            "Response should contain discussion_id: {body}"
        );
        let disc_id = body["data"]["discussion_id"].as_str().unwrap();
        assert!(!disc_id.is_empty(), "discussion_id should not be empty");
    }

    /// Discussions created for validation/bootstrap/briefing should have pin_first_message=true.
    /// This test verifies that a discussion with pin_first_message=true roundtrips correctly
    /// through DB insert and retrieval via the GET API.
    #[tokio::test]
    async fn validation_discussion_has_pin_first_message() {
        let state = test_state();

        // Insert a discussion with pin_first_message=true (simulating what validation creates)
        state
            .db
            .with_conn(|conn| {
                let now = chrono::Utc::now();
                let disc = crate::models::Discussion {
                    awaiting_agent: false,
                    agent_running: false,
                    id: "disc-pin".into(),
                    project_id: None,
                    title: "Validation audit AI".into(),
                    agent: crate::models::AgentType::ClaudeCode,
                    language: "en".into(),
                    participants: vec![crate::models::AgentType::ClaudeCode],
                    message_count: 0,
                    non_system_message_count: 0,
                    messages: vec![],
                    skill_ids: vec![],
                    profile_ids: vec![],
                    directive_ids: vec![],
                    archived: false,
                    pinned: false,
                    workspace_mode: "Direct".into(),
                    workspace_path: None,
                    worktree_branch: None,
                    tier: crate::models::ModelTier::Default,
                    model: None,
                    pin_first_message: true,
                    summary_cache: None,
                    summary_up_to_msg_idx: None,
                    summary_strategy: crate::models::SummaryStrategy::Auto,
                    introspection_call_count: 0,
                    shared_id: None,
                    shared_with: vec![],
                    workflow_run_id: None,
                    test_mode_restore_branch: None,
                    test_mode_stash_ref: None,
                    created_at: now,
                    updated_at: now,
                };
                crate::db::discussions::insert_discussion(conn, &disc)?;
                Ok(())
            })
            .await
            .unwrap();

        // GET the discussion and verify pin_first_message is true
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions/disc-pin")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body["success"].as_bool().unwrap(),
            "GET disc-pin must succeed: {body}"
        );
        assert_eq!(
            body["data"]["pin_first_message"], true,
            "pin_first_message must be true for validation discussions: {body}"
        );

        // Also verify via list endpoint
        let req = Request::builder()
            .method("GET")
            .uri("/api/discussions")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let discs = body["data"].as_array().unwrap();
        let pin_disc = discs.iter().find(|d| d["id"] == "disc-pin").unwrap();
        assert_eq!(
            pin_disc["pin_first_message"], true,
            "pin_first_message must be true in list view too: {pin_disc}"
        );
    }

    #[tokio::test]
    async fn start_briefing_nonexistent_project() {
        let state = test_state();

        let body_json = serde_json::json!({ "agent": "ClaudeCode" });
        let req = Request::builder()
            .method("POST")
            .uri("/api/projects/nonexistent/start-briefing")
            .header("Content-Type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        let (status, body) = send(state, false, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "start-briefing on nonexistent project: {body}"
        );
        assert!(
            !body["success"].as_bool().unwrap_or(true),
            "start-briefing on nonexistent project should return error: {body}"
        );
    }

    // ─── Secret theme unlock ─────────────────────────────────────────────

    /// With a theme/code pair configured locally, the matching code
    /// returns the theme in the unlocks array.
    #[tokio::test]
    async fn theme_unlock_valid_code_returns_theme_name() {
        let state = test_state();
        {
            let mut cfg = state.config.write().await;
            cfg.secret_themes
                .insert("matrix".into(), "alpha-code".into());
            cfg.secret_themes
                .insert("sakura".into(), "beta-code".into());
        }

        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"code": "alpha-code"}).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "unexpected response: {body}");
        let unlocks = body["data"]["unlocks"].as_array().expect("unlocks array");
        assert_eq!(unlocks.len(), 1, "expected 1 unlock, got {body}");
        assert_eq!(unlocks[0]["kind"], "theme");
        assert_eq!(unlocks[0]["name"], "matrix");
    }

    /// Wrong code → generic error, no enumeration of valid themes in
    /// the response (so a brute-forcer can't learn configured names).
    #[tokio::test]
    async fn theme_unlock_wrong_code_returns_generic_error() {
        let state = test_state();
        {
            let mut cfg = state.config.write().await;
            cfg.secret_themes
                .insert("matrix".into(), "alpha-code".into());
        }

        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"code": "wrong-guess"}).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);
        // Error message must not leak configured theme names.
        let err = body["error"].as_str().unwrap_or_default();
        assert!(
            !err.to_lowercase().contains("matrix"),
            "error message leaks theme name: {err}"
        );
    }

    /// Empty / whitespace-only code is rejected up front — no DB lookup.
    #[tokio::test]
    async fn theme_unlock_empty_code_rejected() {
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"code": "   "}).to_string()))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false);
    }

    /// With no `[secret_themes]` table configured, ANY code fails —
    /// proves the default-empty HashMap doesn't accidentally match.
    #[tokio::test]
    async fn theme_unlock_no_codes_configured_always_fails() {
        let state = test_state();
        // Intentionally do not populate secret_themes.
        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"code": "anything"}).to_string(),
            ))
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["success"], false);
    }

    /// End-to-end: matching via the config.toml plaintext fallback
    /// returns the theme in the new unlocks-array shape.
    #[tokio::test]
    async fn theme_unlock_plaintext_fallback_returns_unlocks_array() {
        let state = test_state();
        {
            let mut cfg = state.config.write().await;
            cfg.secret_themes.insert("sakura".into(), "hello".into());
        }
        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::json!({"code": "hello"}).to_string()))
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["success"], true, "unlock path broke: {body}");
        let unlocks = body["data"]["unlocks"].as_array().expect("unlocks array");
        assert_eq!(unlocks.len(), 1);
        assert_eq!(unlocks[0]["kind"], "theme");
        assert_eq!(unlocks[0]["name"], "sakura");
    }

    /// The kronnBatman built-in code is a BUNDLE — single input code
    /// matches two entries (one profile, one theme) and both come back
    /// in the response. Also persists `batman` into
    /// `config.unlocked_profiles` so subsequent /api/profiles includes it.
    #[tokio::test]
    async fn theme_unlock_batman_bundle_unlocks_profile_and_theme() {
        let state = test_state();

        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"code": "kronnBatman"}).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "kronnBatman rejected: {body}");

        let unlocks = body["data"]["unlocks"].as_array().expect("unlocks array");
        assert_eq!(unlocks.len(), 2, "bundle should yield 2 entries: {body}");
        // Order is fixed by array declaration: profile first, theme second
        let kinds: Vec<&str> = unlocks
            .iter()
            .map(|u| u["kind"].as_str().unwrap())
            .collect();
        let names: Vec<&str> = unlocks
            .iter()
            .map(|u| u["name"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"profile"));
        assert!(kinds.contains(&"theme"));
        assert!(names.contains(&"batman"));
        assert!(names.contains(&"gotham"));

        // And the profile was persisted to config.
        let cfg = state.config.read().await;
        assert!(
            cfg.unlocked_profiles.iter().any(|p| p == "batman"),
            "batman must be in unlocked_profiles after unlock, got {:?}",
            cfg.unlocked_profiles
        );
    }

    /// The kronnEuronews built-in code unlocks the single `euronews`
    /// theme (no bundle) via the committed hash — no config.toml needed,
    /// so it works on every self-hosted instance after update.
    #[tokio::test]
    async fn theme_unlock_euronews_built_in_unlocks_theme() {
        let state = test_state();
        // Intentionally leave secret_themes empty — this must match a
        // BUILT_IN_UNLOCK_HASHES entry, not a local plaintext override.
        let req = Request::builder()
            .method("POST")
            .uri("/api/themes/unlock")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({"code": "kronnEuronews"}).to_string(),
            ))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "kronnEuronews rejected: {body}");
        let unlocks = body["data"]["unlocks"].as_array().expect("unlocks array");
        assert_eq!(
            unlocks.len(),
            1,
            "euronews is a single theme, not a bundle: {body}"
        );
        assert_eq!(unlocks[0]["kind"], "theme");
        assert_eq!(unlocks[0]["name"], "euronews");
    }

    /// Batman is hidden from GET /api/profiles until unlocked, and
    /// shows up afterwards. Verifies the secret-profile filter in
    /// both states from the caller perspective.
    #[tokio::test]
    async fn profiles_batman_hidden_until_unlocked() {
        let state = test_state();

        // Pre-unlock: batman NOT in the list.
        let req = Request::builder()
            .method("GET")
            .uri("/api/profiles")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let profiles = body["data"].as_array().expect("profiles array");
        assert!(
            !profiles.iter().any(|p| p["id"] == "batman"),
            "batman leaked in pre-unlock list"
        );

        // Flip the flag manually (same as what unlock does).
        state
            .config
            .write()
            .await
            .unlocked_profiles
            .push("batman".into());

        // Post-unlock: batman visible.
        let req = Request::builder()
            .method("GET")
            .uri("/api/profiles")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        let profiles = body["data"].as_array().expect("profiles array");
        let batman = profiles
            .iter()
            .find(|p| p["id"] == "batman")
            .expect("batman must be visible after unlock");
        // Sanity: frontmatter-derived fields made it through.
        assert_eq!(batman["avatar"], "🦇");
        assert_eq!(batman["color"], "#ffd400");
    }

    /// GET /api/profiles/:id returns "not found" for a locked secret
    /// profile — same error as a truly-missing id, so a probing user
    /// cannot distinguish "locked" from "nonexistent".
    #[tokio::test]
    async fn profile_get_batman_404s_when_locked() {
        let state = test_state();
        // locked
        let req = Request::builder()
            .method("GET")
            .uri("/api/profiles/batman")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(body["success"], false);

        // unlock then fetch
        state
            .config
            .write()
            .await
            .unlocked_profiles
            .push("batman".into());
        let req = Request::builder()
            .method("GET")
            .uri("/api/profiles/batman")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(
            body["success"], true,
            "batman unfetchable post-unlock: {body}"
        );
        assert_eq!(body["data"]["id"], "batman");
    }

    /// Smoke test for `AppState::new_defaults` — proves the factory
    /// produces a state with every runtime field non-empty / present.
    /// This is the guard against the 0.5.0 regression where
    /// `desktop/src-tauri/src/main.rs` was missing a new AppState
    /// field: the factory is the ONLY init path (both mains + every
    /// test use it), so forgetting to default-initialize a new field
    /// here fails this test AND prevents the struct-literal drift.
    #[tokio::test]
    async fn app_state_new_defaults_initializes_every_runtime_field() {
        let db = Arc::new(Database::open_in_memory().expect("in-memory DB"));
        let config_arc = Arc::new(RwLock::new(default_config()));
        let state = AppState::new_defaults(config_arc, db, DEFAULT_MAX_CONCURRENT_AGENTS);

        // Every Arc-wrapped field must be reachable and initialized.
        // Concurrent-initialized containers (HashMap) must be empty —
        // catches accidentally wiring a shared-across-tests singleton.
        assert!(Arc::strong_count(&state.config) >= 1);
        assert!(Arc::strong_count(&state.db) >= 1);
        assert_eq!(
            state.agent_semaphore.available_permits(),
            DEFAULT_MAX_CONCURRENT_AGENTS
        );
        assert_eq!(
            state.audit_tracker.lock().unwrap().progress.len(),
            0,
            "fresh AuditTracker must be empty"
        );
        // ws_broadcast subscriber count starts at 0 — prove the channel
        // is open by subscribing (no panic = sender is alive).
        let _sub = state.ws_broadcast.subscribe();
        assert_eq!(
            state.cancel_registry.lock().unwrap().len(),
            0,
            "cancel registry must start empty"
        );
        assert_eq!(
            state.oauth2_cache.lock().await.len(),
            0,
            "oauth2 cache must start empty"
        );
    }

    // ─── Document generation endpoints ────────────────────────────────

    /// Without a running sidecar, `POST /api/docs/pdf` must return an
    /// end-user action instead of asking someone using an installer to
    /// execute a repository-only Make target.
    #[tokio::test]
    async fn docs_pdf_returns_actionable_error_when_sidecar_absent() {
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/docs/pdf")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "discussion_id": "disc-1",
                    "html": "<html><body>hi</body></html>",
                })
                .to_string(),
            ))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], false, "must fail gracefully: {body}");
        let err = body["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("Update or reinstall Kronn"),
            "error must give an installer-level recovery action, got: {err}"
        );
        assert!(
            !err.contains("make docs-setup"),
            "must not expose a developer command"
        );
    }

    /// Browser-rendered Live Page images commonly exceed axum's default 2 MiB
    /// JSON limit. They must reach the docs handler instead of failing with 413.
    #[tokio::test]
    async fn docs_pdf_accepts_materialized_page_over_default_body_limit() {
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/docs/pdf")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "discussion_id": "page-1",
                    "html": "<html><body>report</body></html>",
                    "page_images": [format!("data:image/png;base64,{}", "A".repeat(2_200_000))],
                })
                .to_string(),
            ))
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "request must pass the route body limit"
        );
        assert_eq!(
            body["success"], false,
            "test state has no docs sidecar: {body}"
        );
        assert!(body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Update or reinstall Kronn"));
    }

    /// Reject requests with traversal payloads in `discussion_id` before
    /// they touch the filesystem. The handler returns a generic
    /// "invalid discussion_id" rather than leaking path details.
    #[tokio::test]
    async fn docs_pdf_rejects_traversal_in_discussion_id() {
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/docs/pdf")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "discussion_id": "../../etc",
                    "html": "<html></html>",
                })
                .to_string(),
            ))
            .unwrap();
        let (_, body) = send(state, false, req).await;
        assert_eq!(body["success"], false);
        // Either the sidecar-absent error fires first (no venv in
        // tests) OR the traversal guard does. Both are acceptable —
        // the key property is we never let a traversing discussion_id
        // get as far as path building.
        let err = body["error"].as_str().unwrap_or_default();
        assert!(
            err.contains("Update or reinstall Kronn") || err.contains("invalid discussion_id"),
            "expected sidecar-absent OR traversal-guard error, got: {err}"
        );
    }

    /// GET /api/docs/file/:disc/:filename rejects path-traversal payloads
    /// BEFORE attempting disk access. Defense in depth on top of the
    /// canonicalize guard.
    #[tokio::test]
    async fn docs_download_rejects_traversal_filename() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/docs/file/disc-1/..%2F..%2Fetc%2Fpasswd")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(state, false, req).await;
        // URL decoder would land `..` in the filename → handler rejects.
        // We don't assert the exact status code (400 vs 404 depends on
        // whether the router URL-decoded the %2F); either is safe.
        assert!(
            status.is_client_error() || status.is_server_error(),
            "traversal filename must not succeed: {status}"
        );
    }

    /// Each of the 5 document-generation endpoints must surface the
    /// same actionable reinstall/update error when the document
    /// sidecar isn't running. Parametrized so a single matrix covers
    /// all formats and catches a new endpoint silently forgetting the
    /// sidecar check.
    #[tokio::test]
    async fn docs_all_formats_return_actionable_error_when_sidecar_absent() {
        let cases: &[(&str, serde_json::Value)] = &[
            (
                "pdf",
                serde_json::json!({"discussion_id": "d", "html": "<p>x</p>"}),
            ),
            (
                "docx",
                serde_json::json!({"discussion_id": "d", "html": "<p>x</p>"}),
            ),
            (
                "xlsx",
                serde_json::json!({"discussion_id": "d", "sheets": [{"name": "S", "rows": [["a"]]}]}),
            ),
            (
                "csv",
                serde_json::json!({"discussion_id": "d", "rows": [["a", "b"]]}),
            ),
            (
                "pptx",
                serde_json::json!({"discussion_id": "d", "slides": [{"title": "Hi"}]}),
            ),
        ];
        for (fmt, body) in cases {
            let state = test_state();
            let req = Request::builder()
                .method("POST")
                .uri(format!("/api/docs/{fmt}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let (status, resp) = send(state, false, req).await;
            assert_eq!(status, StatusCode::OK, "[{fmt}]");
            assert_eq!(
                resp["success"], false,
                "[{fmt}] should fail without sidecar: {resp}"
            );
            let err = resp["error"].as_str().unwrap_or_default();
            assert!(
                err.contains("Update or reinstall Kronn"),
                "[{fmt}] error must give an installer-level recovery action, got: {err}"
            );
            assert!(
                !err.contains("make docs-setup"),
                "[{fmt}] leaked a developer command"
            );
        }
    }

    /// GET /api/docs/file/:disc/:filename returns 404 for files that
    /// don't exist — the download handler must not leak existence
    /// information about other discussions' files either.
    #[tokio::test]
    async fn docs_download_404_when_file_missing() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/docs/file/disc-nonexistent/never.pdf")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(state, false, req).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ─── Auto-trigger opt-out (GET endpoint edge cases) ─────────────

    /// The GET endpoint returns an empty array when no skill has been
    /// opted out — fresh install path.
    #[tokio::test]
    async fn auto_trigger_list_empty_by_default() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/skills/auto-triggers/disabled")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        let arr = body["data"].as_array().expect("data array");
        assert!(arr.is_empty(), "no skills opted out by default: {body}");
    }

    /// Toggling a skill id that doesn't exist in the registry still
    /// succeeds — the config stores arbitrary strings, checking
    /// existence is the frontend's job. Guards against a race where
    /// the frontend toggles a skill that was just deleted.
    #[tokio::test]
    #[serial]
    async fn auto_trigger_toggle_unknown_skill_still_works() {
        isolate_config_dir();
        let state = test_state();
        let req = Request::builder()
            .method("POST")
            .uri("/api/skills/some-made-up-skill/auto-trigger/toggle")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], true, "toggle to ON must succeed: {body}");
    }

    /// The auto-trigger toggle flips in-memory state AND persists it.
    /// Sending the toggle twice ends at the original state (idempotent
    /// round-trip), and the GET endpoint returns the live list.
    #[tokio::test]
    #[serial]
    async fn auto_trigger_toggle_flips_and_persists() {
        isolate_config_dir();
        let state = test_state();

        // First toggle on kronn-docs → disables.
        let req = Request::builder()
            .method("POST")
            .uri("/api/skills/kronn-docs/auto-trigger/toggle")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"], true,
            "first toggle must report disabled=true: {body}"
        );
        assert!(
            state
                .config
                .read()
                .await
                .disabled_auto_skills
                .iter()
                .any(|s| s == "kronn-docs"),
            "config must now list kronn-docs as disabled"
        );

        // GET reports the current list.
        let req = Request::builder()
            .method("GET")
            .uri("/api/skills/auto-triggers/disabled")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let ids = body["data"].as_array().expect("data array");
        assert!(ids.iter().any(|v| v == "kronn-docs"));

        // Second toggle → re-enables (opt-out removed).
        let req = Request::builder()
            .method("POST")
            .uri("/api/skills/kronn-docs/auto-trigger/toggle")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        assert_eq!(
            body["data"], false,
            "second toggle must report disabled=false: {body}"
        );
        assert!(
            state.config.read().await.disabled_auto_skills.is_empty(),
            "list must be empty again"
        );
    }

    // ─── 0.8.3 — Bundle endpoint (KRONN:BUNDLE_READY) ────────────────────────

    /// Happy path: a bundle with 1 Quick Prompt + 1 Quick API +
    /// 1 Workflow, the workflow referencing both via `@bundle:` ids.
    /// All artifacts must land in DB and the workflow steps must
    /// have the real ids substituted in.
    #[tokio::test]
    async fn bundle_creates_qp_qa_and_workflow_atomically() {
        let state = test_state();

        let body = serde_json::json!({
            "quick_prompts": [{
                "bundle_id": "summarize",
                "name": "Summarize one item",
                "icon": "📝",
                "prompt_template": "Summarize {{batch.item.title}} in 2 sentences.",
                "agent": "ClaudeCode",
                "description": "Per-item summarizer for daily digest"
            }],
            "quick_apis": [{
                "bundle_id": "fetch-cb",
                "name": "Top pages",
                "icon": "📊",
                "api_plugin_slug": "chartbeat",
                "api_config_id": "stub-cfg",
                "api_endpoint_path": "/v3/historical/topPages.json",
                "api_method": "GET"
            }],
            "workflow": {
                "name": "Daily digest",
                "trigger": { "type": "Manual" },
                "steps": [
                    {
                        "name": "fetch",
                        "step_type": { "type": "ApiCall" },
                        "agent": "ClaudeCode",
                        "prompt_template": "",
                        "mode": { "type": "Normal" },
                        "quick_api_id": "@bundle:fetch-cb"
                    },
                    {
                        "name": "summarize_each",
                        "step_type": { "type": "BatchQuickPrompt" },
                        "agent": "ClaudeCode",
                        "prompt_template": "",
                        "mode": { "type": "Normal" },
                        "batch_quick_prompt_id": "@bundle:summarize",
                        "batch_items_from": "{{steps.fetch.data}}"
                    }
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows/bundle")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (status, resp) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK, "bundle endpoint should 200: {resp}");
        assert_eq!(resp["success"], true, "bundle should succeed: {resp}");

        let data = &resp["data"];
        let qp_id = data["quick_prompts"][0]["id"]
            .as_str()
            .expect("qp id")
            .to_string();
        let qa_id = data["quick_apis"][0]["id"]
            .as_str()
            .expect("qa id")
            .to_string();
        let wf_id = data["workflow"]["id"].as_str().expect("wf id").to_string();
        assert!(!qp_id.is_empty() && !qa_id.is_empty() && !wf_id.is_empty());
        assert_eq!(data["quick_prompts"][0]["bundle_id"], "summarize");
        assert_eq!(data["quick_apis"][0]["bundle_id"], "fetch-cb");

        // Round-trip: GET the workflow and verify substitution happened.
        let req = Request::builder()
            .method("GET")
            .uri(format!("/api/workflows/{}", wf_id))
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let steps = &body["data"]["steps"];
        assert_eq!(
            steps[0]["quick_api_id"], qa_id,
            "step `fetch` quick_api_id must have been substituted from @bundle:fetch-cb to {qa_id}"
        );
        assert_eq!(steps[1]["batch_quick_prompt_id"], qp_id,
            "step `summarize_each` batch_quick_prompt_id must have been substituted from @bundle:summarize to {qp_id}");

        // Double-check the QP and QA are listable independently.
        let req = Request::builder()
            .method("GET")
            .uri("/api/quick-prompts")
            .body(Body::empty())
            .unwrap();
        let (_, qps) = send(state.clone(), false, req).await;
        assert!(
            qps["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == qp_id),
            "created QP must appear in /api/quick-prompts"
        );
        let req = Request::builder()
            .method("GET")
            .uri("/api/quick-apis")
            .body(Body::empty())
            .unwrap();
        let (_, qas) = send(state.clone(), false, req).await;
        assert!(
            qas["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == qa_id),
            "created QA must appear in /api/quick-apis"
        );
    }

    /// Validation: a workflow referencing `@bundle:nonexistent` must
    /// 200 + `success: false` (errors flow through `ApiResponse`),
    /// with NO artifacts created. The error message must name the
    /// missing ref so the user (or the calling agent) can fix the
    /// payload.
    #[tokio::test]
    async fn bundle_rejects_unknown_bundle_ref_and_creates_nothing() {
        let state = test_state();
        let body = serde_json::json!({
            "quick_prompts": [{
                "bundle_id": "summarize",
                "name": "QP",
                "prompt_template": "..."
            }],
            "workflow": {
                "name": "Broken",
                "trigger": { "type": "Manual" },
                "steps": [
                    {
                        "name": "fetch",
                        "step_type": { "type": "Agent" },
                        "agent": "ClaudeCode",
                        "prompt_template": "test",
                        "mode": { "type": "Normal" },
                        "quick_prompt_id": "@bundle:NONEXISTENT"
                    }
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows/bundle")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (_, resp) = send(state.clone(), false, req).await;
        assert_eq!(resp["success"], false, "must reject: {resp}");
        let err = resp["error"].as_str().unwrap_or("");
        assert!(
            err.contains("NONEXISTENT") || err.contains("unknown bundle_id"),
            "error message must surface the missing ref: {err}"
        );

        // Negative side-effect check: the QP we declared must NOT
        // have landed in DB (the validator runs before any insert).
        let req = Request::builder()
            .method("GET")
            .uri("/api/quick-prompts")
            .body(Body::empty())
            .unwrap();
        let (_, qps) = send(state.clone(), false, req).await;
        let arr = qps["data"].as_array().unwrap();
        assert!(
            arr.is_empty(),
            "no QP must be created when the workflow validation fails: got {} rows",
            arr.len()
        );
    }

    /// Validation: duplicate bundle_id across categories must be
    /// rejected. We don't want `@bundle:foo` to resolve ambiguously.
    #[tokio::test]
    async fn bundle_rejects_duplicate_bundle_id_across_categories() {
        let state = test_state();
        let body = serde_json::json!({
            "quick_prompts": [{ "bundle_id": "dup", "name": "QP", "prompt_template": "x" }],
            "quick_apis":    [{
                "bundle_id": "dup",
                "name": "QA",
                "api_plugin_slug": "chartbeat",
                "api_config_id": "stub",
                "api_endpoint_path": "/x"
            }],
            "workflow": {
                "name": "Doesn't matter",
                "trigger": { "type": "Manual" },
                "steps": [
                    { "name": "s1", "step_type": { "type": "Agent" }, "agent": "ClaudeCode",
                      "prompt_template": "x", "mode": { "type": "Normal" } }
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows/bundle")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (_, resp) = send(state, false, req).await;
        assert_eq!(
            resp["success"], false,
            "must reject duplicate bundle_id: {resp}"
        );
        assert!(
            resp["error"].as_str().unwrap_or("").contains("Duplicate"),
            "error must say 'Duplicate': {}",
            resp["error"]
        );
    }

    /// 0.8.3 — Linked repos PUT happy path. Creates a project,
    /// PUTs a list of 2 companion repos, GETs the project back and
    /// asserts the list round-trips. Locks the canonical
    /// "atomic-replace via PUT" semantics so a future per-row
    /// CRUD refactor can't silently change the wire shape.
    #[tokio::test]
    async fn linked_repos_put_round_trips_via_get_project() {
        let state = test_state();

        // Spawn a project directly in DB (avoids the bootstrap flow
        // which needs filesystem state we don't want to stub here).
        let project_path = std::env::temp_dir()
            .join(format!("kronn-test-linked-{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();
        std::fs::create_dir_all(&project_path).expect("test project dir");
        let pid = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let project = crate::models::Project {
            id: pid.clone(),
            name: "test-with-linked".into(),
            path: project_path.clone(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: Default::default(),
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
            .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
            .await
            .expect("insert project");

        let body = serde_json::json!([
            { "id": "lr-1", "name": "backend-api", "kind": "api",
              "location": "/home/priol/Repos/backend-api",
              "description": "GraphQL schema for the frontend" },
            { "id": "lr-2", "name": "infra", "kind": "iac",
              "location": "https://github.com/org/infra",
              "description": "" }
        ]);
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/projects/{}/linked-repos", pid))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (status, resp) = send(state.clone(), false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            resp["success"], true,
            "PUT linked-repos must succeed: {resp}"
        );

        // Round-trip via GET /api/projects (list endpoint returns
        // `linked_repos` on each Project — the list view feeds the
        // ProjectCard which is where the user reads them back).
        let req = Request::builder()
            .method("GET")
            .uri("/api/projects")
            .body(Body::empty())
            .unwrap();
        let (_, body) = send(state.clone(), false, req).await;
        let proj = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == pid)
            .expect("project must be in list");
        let linked = proj["linked_repos"].as_array().expect("linked_repos array");
        assert_eq!(linked.len(), 2);
        assert_eq!(linked[0]["name"], "backend-api");
        assert_eq!(linked[0]["kind"], "api");
        assert_eq!(linked[1]["name"], "infra");
        let _ = std::fs::remove_dir_all(&project_path);
    }

    /// Validation: a linked repo with an unknown `kind` must be
    /// rejected (so the UI picker and the prompt formatter can rely
    /// on a closed set of values without runtime surprises).
    #[tokio::test]
    async fn linked_repos_put_rejects_unknown_kind() {
        let state = test_state();
        let pid = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now();
        let project = crate::models::Project {
            id: pid.clone(),
            name: "p".into(),
            path: "/tmp/p".into(),
            repo_url: None,
            token_override: None,
            ai_config: crate::models::AiConfigStatus {
                detected: false,
                configs: vec![],
            },
            audit_status: Default::default(),
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
            .with_conn(move |conn| crate::db::projects::insert_project(conn, &project))
            .await
            .unwrap();

        let body = serde_json::json!([
            { "id": "lr-1", "name": "weird", "kind": "blockchain-mainframe", "location": "/x" }
        ]);
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/api/projects/{}/linked-repos", pid))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (_, resp) = send(state, false, req).await;
        assert_eq!(
            resp["success"], false,
            "unknown kind must be rejected: {resp}"
        );
        assert!(
            resp["error"].as_str().unwrap_or("").contains("kind"),
            "error message must mention `kind`: {}",
            resp["error"]
        );
    }

    /// Empty bundle (no QP/QA, just a workflow) is allowed — behaves
    /// like a regular `POST /api/workflows`. Locks the principle
    /// "bundle is a superset, not a different endpoint shape".
    #[tokio::test]
    async fn bundle_with_only_a_workflow_succeeds() {
        let state = test_state();
        let body = serde_json::json!({
            "workflow": {
                "name": "Just a workflow",
                "trigger": { "type": "Manual" },
                "steps": [
                    { "name": "s1", "step_type": { "type": "Agent" }, "agent": "ClaudeCode",
                      "prompt_template": "Hi", "mode": { "type": "Normal" } }
                ]
            }
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/workflows/bundle")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let (status, resp) = send(state, false, req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp["success"], true, "empty bundle must succeed: {resp}");
        assert!(resp["data"]["quick_prompts"].as_array().unwrap().is_empty());
        assert!(resp["data"]["quick_apis"].as_array().unwrap().is_empty());
        assert!(resp["data"]["workflow"]["id"].as_str().is_some());
    }

    // ─── 0.8.7 — GET /api/conventions/agents-md-format-v1 ──────────────────
    //
    // The Settings → Sourcing section links here to render the local spec.
    // A constant-only test (in setup.rs) doesn't catch a router-misregistration
    // regression that would silently 404. This pins the full route.

    #[tokio::test]
    async fn conventions_route_returns_markdown_spec() {
        let state = test_state();
        let req = Request::builder()
            .method("GET")
            .uri("/api/conventions/agents-md-format-v1")
            .body(Body::empty())
            .unwrap();
        let app = build_router_with_auth(state, false);
        let resp = app.oneshot(req).await.expect("oneshot failed");
        assert_eq!(resp.status(), StatusCode::OK);

        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(
            ct.starts_with("text/markdown"),
            "expected text/markdown content-type, got {ct:?}",
        );

        let bytes = resp
            .into_body()
            .collect()
            .await
            .expect("body collect")
            .to_bytes();
        let body = std::str::from_utf8(&bytes).expect("utf-8 body");
        assert!(body.contains("Kronn `AGENTS.md` convention"));
        assert!(body.contains("kronn:doc-version"));
        assert_eq!(
            body,
            crate::core::anti_halluc::SPEC_AGENTS_MD_V1,
            "route body must match the embedded constant byte-for-byte",
        );
    }
    // ─── KT-255: the cross-agent memory survives the UI removal ─────────────
    //
    // The manual binding FORM is gone from the frontend. The routes behind it are
    // not: `source_agent` / `source_session_id` is what lets Codex pick up a
    // discussion Claude started, and what a restarting bridge uses to find its own
    // room again. Removing the form is a UX change; these tests are what makes it
    // provable that it was only a UX change.

    /// Create a discussion straight in the DB, without going through an agent.
    async fn seed_disc(state: &AppState, id: &str) {
        let owned = id.to_string();
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO discussions (id, title, agent, language, created_at, updated_at)
                     VALUES (?1, 'seeded', 'ClaudeCode', 'fr', ?2, ?2)",
                    rusqlite::params![owned, now],
                )?;
                Ok(())
            })
            .await
            .expect("seed discussion");
    }

    #[tokio::test]
    async fn disc_link_and_find_by_session_still_answer_without_the_form() {
        // The round trip a restarting CLI depends on: bind, then find the room
        // again from nothing but the session id.
        let state = test_state();
        seed_disc(&state, "disc-link-a").await;

        let (status, body) = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"disc_id":"disc-link-a","source_agent":"Codex",
                        "source_session_id":"codex-session-42"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "link refused: {body}");

        let (status, body) = send(
            state,
            false,
            Request::builder()
                .uri("/api/disc/find_by_session?source_agent=Codex&source_session_id=codex-session-42")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["disc_id"], "disc-link-a",
            "a restarting CLI could not find its room: {body}"
        );
    }

    #[tokio::test]
    async fn disc_unlink_still_clears_a_stale_binding() {
        // The one repair path the UI kept. If this route stopped working, the
        // remaining button would be a lie.
        let state = test_state();
        seed_disc(&state, "disc-unlink-a").await;
        let _ = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"disc_id":"disc-unlink-a","source_agent":"Codex",
                        "source_session_id":"codex-stale"}"#,
                ))
                .unwrap(),
        )
        .await;

        let (status, body) = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/unlink")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"disc_id":"disc-unlink-a"}"#))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true, "unlink refused: {body}");

        let (_, body) = send(
            state,
            false,
            Request::builder()
                .uri("/api/disc/find_by_session?source_agent=Codex&source_session_id=codex-stale")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert!(
            body["data"].is_null() || body["data"]["disc_id"].is_null(),
            "the binding survived the unlink: {body}"
        );
    }

    #[tokio::test]
    async fn a_read_only_ui_still_gets_the_binding_it_displays() {
        // What the header now shows is served by this route alone. It has to carry
        // the agent and the session id, or the read-only view has nothing to say.
        let state = test_state();
        seed_disc(&state, "disc-detail-a").await;
        let _ = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"disc_id":"disc-detail-a","source_agent":"ClaudeCode",
                        "source_session_id":"claude-session-full-id"}"#,
                ))
                .unwrap(),
        )
        .await;

        let (status, body) = send(
            state,
            false,
            Request::builder()
                .uri("/api/discussions/disc-detail-a/source")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["current"]["source_agent"], "ClaudeCode");
        assert_eq!(
            body["data"]["current"]["source_session_id"],
            "claude-session-full-id"
        );
    }

    #[tokio::test]
    async fn linking_a_session_owned_elsewhere_is_refused_without_force() {
        // The protection the removed form implemented in the CLIENT. Now that no
        // client checks first, the SERVER has to be the one refusing — otherwise
        // deleting the form would have deleted the safeguard with it.
        let state = test_state();
        seed_disc(&state, "disc-owner").await;
        seed_disc(&state, "disc-thief").await;
        let _ = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"disc_id":"disc-owner","source_agent":"Codex",
                        "source_session_id":"contested"}"#,
                ))
                .unwrap(),
        )
        .await;

        let (_, body) = send(
            state.clone(),
            false,
            Request::builder()
                .method("POST")
                .uri("/api/disc/link")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"disc_id":"disc-thief","source_agent":"Codex",
                        "source_session_id":"contested"}"#,
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(body["success"], false, "a live thread was stolen: {body}");

        let (_, body) = send(
            state,
            false,
            Request::builder()
                .uri("/api/disc/find_by_session?source_agent=Codex&source_session_id=contested")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            body["data"]["disc_id"], "disc-owner",
            "ownership moved anyway: {body}"
        );
    }

    // ─── HTTP-agent tool executor (TD-20260808) ──────────────────────────────
    //
    // The loop and the model's comprehension were proven separately; this
    // covers the link between them — that the executor actually drives the
    // real handlers and returns something a model can consume.

    #[tokio::test]
    async fn tool_executor_runs_real_handlers_and_projects_the_result() {
        use crate::agents::tools::{ToolCall, ToolExecutor};
        use crate::api::agent_tools::KronnToolExecutor;

        let exec = KronnToolExecutor::new(test_state(), None);
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args,
        };

        // Empty instance: the tools must still answer with a well-formed,
        // empty result — a model that gets an error here would retry forever.
        let out = exec.execute(&call("mcp_list", serde_json::json!({}))).await;
        assert!(out.ok, "mcp_list failed: {:?}", out.content);
        assert!(out.content["plugins"].is_array());
        assert!(
            out.content["next"].as_str().is_some(),
            "the result must tell the model what to do next"
        );

        let out = exec.execute(&call("qa_list", serde_json::json!({}))).await;
        assert!(out.ok, "qa_list failed: {:?}", out.content);
        assert!(out.content["quick_apis"].is_array());
    }

    #[tokio::test]
    async fn tool_executor_reports_bad_input_as_data_not_as_a_dead_turn() {
        use crate::agents::tools::{ToolCall, ToolExecutor};
        use crate::api::agent_tools::KronnToolExecutor;

        let exec = KronnToolExecutor::new(test_state(), None);
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args,
        };

        // Every one of these is a mistake a model actually makes. Each must
        // come back as a readable error it can correct from, never a panic
        // and never a silent empty success.
        for (name, args, expect) in [
            ("qa_run", serde_json::json!({}), "quick_api_id"),
            (
                "plan_get",
                serde_json::json!({}),
                "inside a Kronn discussion",
            ),
            (
                "task_create",
                serde_json::json!({"title": "x"}),
                "inside a Kronn discussion",
            ),
            ("task_get", serde_json::json!({}), "task_id"),
            ("api_endpoints", serde_json::json!({}), "api_plugin_slug"),
            (
                "api_call",
                serde_json::json!({"endpoint_path": "/x"}),
                "api_plugin_slug",
            ),
            ("nope", serde_json::json!({}), "unknown tool"),
        ] {
            let out = exec.execute(&call(name, args)).await;
            assert!(!out.ok, "{name} should have failed");
            let msg = out.content["error"].as_str().unwrap_or_default();
            assert!(
                msg.contains(expect),
                "{name}: error should mention `{expect}`, got: {msg}"
            );
        }

        // A slug that does not exist must say so rather than return an empty
        // endpoint list, which a model would read as "this plugin has none".
        let out = exec
            .execute(&call(
                "api_endpoints",
                serde_json::json!({"api_plugin_slug": "ghost"}),
            ))
            .await;
        assert!(!out.ok);
        assert!(out.content["error"]
            .as_str()
            .unwrap_or_default()
            .contains("ghost"));
    }

    /// KT-407 — Qwen emitted valid line numbers as JSON strings during a real
    /// worker repair. `read_file` already normalized that representation; the
    /// edit boundary must do the same or a correct CAS edit is refused forever.
    #[tokio::test]
    async fn http_agent_edit_lines_accepts_quoted_decimal_line_numbers() {
        use crate::agents::tools::ToolCall;
        use crate::api::agent_tools::KronnToolExecutor;

        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        std::fs::write(project_dir.path().join("answer.txt"), "first\nold\nthird\n")
            .expect("seed editable file");
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-quoted-edit-lines', 'Quoted edit lines', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                Ok(())
            })
            .await
            .expect("seed project");

        let exec = KronnToolExecutor::workflow_arc(
            state,
            Some("project-quoted-edit-lines".into()),
            "run-quoted-edit-lines".into(),
            "edit".into(),
        );
        let call = |name: &str, arguments: serde_json::Value| ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments,
        };
        let read = exec
            .execute(&call(
                "read_file",
                serde_json::json!({"path": "answer.txt"}),
            ))
            .await;
        assert!(read.ok, "read_file failed: {:?}", read.content);
        let expected_sha256 = read.content["content_sha256"]
            .as_str()
            .expect("read receipt must include its SHA-256");

        let missing_start = exec
            .execute(&call(
                "edit_lines",
                serde_json::json!({
                    "path": "answer.txt",
                    "end_line": "2",
                    "new_string": "new",
                    "expected_sha256": expected_sha256,
                }),
            ))
            .await;
        assert!(!missing_start.ok);
        assert_eq!(
            missing_start.content["error"],
            "missing required field `start_line`"
        );

        let invalid_start = exec
            .execute(&call(
                "edit_lines",
                serde_json::json!({
                    "path": "answer.txt",
                    "start_line": "first",
                    "end_line": "2",
                    "new_string": "new",
                    "expected_sha256": expected_sha256,
                }),
            ))
            .await;
        assert!(!invalid_start.ok);
        assert_eq!(
            invalid_start.content["error"],
            "`start_line` must be a positive integer"
        );

        let edited = exec
            .execute(&call(
                "edit_lines",
                serde_json::json!({
                    "path": "answer.txt",
                    "start_line": "2",
                    "end_line": " 2 ",
                    "new_string": "new",
                    "expected_sha256": expected_sha256,
                }),
            ))
            .await;
        assert!(
            edited.ok,
            "quoted line numbers were refused: {:?}",
            edited.content
        );
        assert_eq!(
            std::fs::read_to_string(project_dir.path().join("answer.txt")).unwrap(),
            "first\nnew\nthird\n"
        );
    }

    #[tokio::test]
    async fn http_agent_insert_after_line_preserves_the_anchor_and_uses_cas() {
        use crate::agents::tools::ToolCall;
        use crate::api::agent_tools::KronnToolExecutor;

        let state = test_state();
        let project_dir = tempfile::TempDir::new().expect("temporary project directory");
        std::fs::write(
            project_dir.path().join("guide.md"),
            "before\nANCHOR\nafter\n",
        )
        .expect("seed insertion target");
        let project_path = project_dir.path().to_string_lossy().into_owned();
        state
            .db
            .with_conn(move |connection| {
                connection.execute(
                    "INSERT INTO projects
                     (id, name, path, ai_config_json, created_at, updated_at)
                     VALUES ('project-insert-after-line', 'Insert after line', ?1, '{}', 'now', 'now')",
                    [&project_path],
                )?;
                Ok(())
            })
            .await
            .expect("seed project");

        let exec = KronnToolExecutor::workflow_arc(
            state,
            Some("project-insert-after-line".into()),
            "run-insert-after-line".into(),
            "insert".into(),
        );
        let call = |name: &str, arguments: serde_json::Value| ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments,
        };
        assert!(exec
            .catalogue()
            .iter()
            .any(|tool| tool["function"]["name"] == "insert_after_line"));
        let read = exec
            .execute(&call(
                "read_file",
                serde_json::json!({"path": "guide.md", "offset": 2, "limit": 1}),
            ))
            .await;
        assert!(read.ok, "read_file failed: {:?}", read.content);
        let receipt = read.content["content_sha256"]
            .as_str()
            .expect("read receipt")
            .to_string();

        let missing_anchor = exec
            .execute(&call(
                "insert_after_line",
                serde_json::json!({
                    "path": "guide.md",
                    "new_string": "new paragraph",
                    "expected_sha256": receipt,
                }),
            ))
            .await;
        assert!(!missing_anchor.ok);
        assert_eq!(
            missing_anchor.content["error"],
            "missing required field `anchor_line`"
        );

        let inserted = exec
            .execute(&call(
                "insert_after_line",
                serde_json::json!({
                    "path": "guide.md",
                    "anchor_line": " 2 ",
                    "new_string": "new paragraph",
                    "expected_sha256": receipt,
                }),
            ))
            .await;
        assert!(inserted.ok, "insertion failed: {:?}", inserted.content);
        assert_eq!(
            inserted.content["anchor_preserved"],
            serde_json::json!(true)
        );
        assert_eq!(
            std::fs::read_to_string(project_dir.path().join("guide.md")).unwrap(),
            "before\nANCHOR\nnew paragraph\nafter\n"
        );
    }

    #[tokio::test]
    async fn http_agent_planning_tools_create_idempotently_in_the_current_discussion() {
        use crate::agents::tools::ToolCall;
        use crate::api::agent_tools::KronnToolExecutor;

        let state = test_state();
        let discussion_id = "http-agent-planning-disc";
        let disc = crate::models::Discussion {
            awaiting_agent: false,
            agent_running: false,
            id: discussion_id.into(),
            project_id: None,
            title: "Planning from Ollama".into(),
            agent: crate::models::AgentType::Ollama,
            language: "en".into(),
            participants: Vec::new(),
            messages: Vec::new(),
            message_count: 0,
            non_system_message_count: 0,
            skill_ids: Vec::new(),
            profile_ids: Vec::new(),
            directive_ids: Vec::new(),
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: crate::models::ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
            shared_id: None,
            shared_with: Vec::new(),
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let seeded = disc.clone();
        state
            .db
            .with_conn(move |conn| crate::db::discussions::insert_discussion(conn, &seeded))
            .await
            .expect("seed discussion");
        let trigger = crate::models::DiscussionMessage {
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
            id: "user-message-1".into(),
            role: crate::models::MessageRole::User,
            channel: crate::models::MessageChannel::Main,
            content: "Create the release plan".into(),
            agent_type: None,
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
        state
            .db
            .with_conn(move |conn| {
                crate::db::discussions::insert_message(conn, discussion_id, &trigger)
            })
            .await
            .expect("seed trigger message");

        let exec = KronnToolExecutor::arc(
            state.clone(),
            Some(discussion_id.into()),
            crate::models::AgentType::Ollama,
            Some("user-message-1".into()),
            None,
        );
        let call = |name: &str, args: serde_json::Value| ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments: args,
        };

        let native_catalogue = exec.catalogue();
        let native_names: Vec<&str> = native_catalogue
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str())
            .collect();
        for lifecycle in [
            "agent_list",
            "task_exec_prepare",
            "task_exec_launch",
            "task_exec_status",
            "task_exec_deliver",
            "task_exec_review",
            "task_exec_cancel",
            "task_exec_reassign",
        ] {
            assert!(
                native_names.contains(&lifecycle),
                "native discussion catalogue is missing {lifecycle}: {native_names:?}"
            );
        }
        for continuation in [
            "agent_job_start",
            "agent_schedule_wake",
            "agent_resume_status",
            "agent_resume_cancel",
        ] {
            assert!(
                native_names.contains(&continuation),
                "native discussion catalogue is missing {continuation}: {native_names:?}"
            );
        }

        let wake_args = serde_json::json!({
            "delay_seconds": 60,
            "reason": "wait for external CI",
            "dedupe_key": "external-ci-42"
        });
        let wake = exec
            .execute(&call("agent_schedule_wake", wake_args.clone()))
            .await;
        assert!(wake.ok, "schedule wake failed: {:?}", wake.content);
        let wake_id = wake.content["id"].as_str().unwrap().to_string();
        let replay = exec.execute(&call("agent_schedule_wake", wake_args)).await;
        assert_eq!(replay.content["id"], wake_id, "wake must dedupe");
        let status = exec
            .execute(&call(
                "agent_resume_status",
                serde_json::json!({"job_id": wake_id}),
            ))
            .await;
        assert_eq!(status.content["jobs"][0]["status"], "pending");
        let cancelled = exec
            .execute(&call(
                "agent_resume_cancel",
                serde_json::json!({"job_id": wake_id}),
            ))
            .await;
        assert_eq!(cancelled.content["cancelled"], true);

        let initial = exec.execute(&call("plan_get", serde_json::json!({}))).await;
        assert!(initial.ok, "plan_get failed: {:?}", initial.content);
        assert_eq!(initial.content["discussion_id"], discussion_id);

        let create_args = serde_json::json!({
            "title": "Write the release plan",
            "status": "todo",
            "priority": "high",
            "idempotency_key": "release-plan-day-one",
            // A model must never be able to escape the executor's discussion.
            "discussion_id": "another-discussion"
        });
        let created = exec
            .execute(&call("task_create", create_args.clone()))
            .await;
        assert!(created.ok, "task_create failed: {:?}", created.content);
        let task_id = created.content["id"].as_str().expect("created task id");
        assert_eq!(created.content["title"], "Write the release plan");

        let retried = exec.execute(&call("task_create", create_args)).await;
        assert!(retried.ok, "idempotent retry failed: {:?}", retried.content);
        assert_eq!(retried.content["id"], task_id);

        let plan = exec.execute(&call("plan_get", serde_json::json!({}))).await;
        assert!(plan.ok, "plan_get after create failed: {:?}", plan.content);
        assert_eq!(plan.content["active"].as_array().map(Vec::len), Some(1));
        assert_eq!(plan.content["active"][0]["task"]["id"], task_id);

        let listed = exec
            .execute(&call(
                "task_list",
                serde_json::json!({
                    // A stale/hallucinated room id cannot hide the current
                    // room's tasks or broaden the native agent's scope.
                    "discussion_id": "another-discussion"
                }),
            ))
            .await;
        assert!(listed.ok, "task_list failed: {:?}", listed.content);
        assert_eq!(listed.content["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(listed.content["items"][0]["id"], task_id);

        let detail = exec
            .execute(&call("task_get", serde_json::json!({"task_id": task_id})))
            .await;
        assert!(detail.ok, "task_get failed: {:?}", detail.content);
        assert_eq!(detail.content["events"][0]["actor_kind"], "agent");
        assert_eq!(detail.content["events"][0]["actor_id"], "Ollama");
        assert_eq!(
            detail.content["events"][0]["actor_session_id"],
            format!("native:{discussion_id}:user-message-1")
        );
        assert_eq!(
            detail.content["events"][0]["source_message_id"],
            "user-message-1"
        );
    }
    /// `api_call` needs a config id, and asking the MODEL to carry it proved
    /// unreliable (a 4B paired `api-speedcurve` with Resend's id, 2026-08-09).
    /// Kronn resolves it from the slug instead — so that mapping must be right.
    #[tokio::test]
    async fn a_worker_room_is_not_offered_the_backlog_it_has_no_business_touching() {
        // KT-398, found by delegating a real task to a local model. The worker
        // got 22 tools, nine of them planning-management. Its first call failed
        // on a missing argument and it fell back to what was on offer:
        // `task_list`, twelve times, until the per-tool budget cut it off. It
        // never opened a file. Removing those tools is not a guard against a bad
        // model — it is refusing to offer a wrong turn.
        let state = test_state();
        let discussion_id = "disc-worker-room";
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES (?1, 'Worker room', ?2, ?2)",
                    rusqlite::params![discussion_id, now],
                )?;
                Ok(())
            })
            .await
            .expect("seed worker room");

        let worker = crate::api::agent_tools::KronnToolExecutor::arc_for_worker_room(
            state.clone(),
            Some(discussion_id.into()),
            crate::models::AgentType::Ollama,
            None,
            None,
            None,
        );
        let names: Vec<String> = worker
            .catalogue()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();

        for absent in [
            "task_list",
            "task_create",
            "plan_get",
            "task_update",
            "task_update_dod",
            "task_link_discussion",
            "task_add_blocker",
            "task_remove_blocker",
            "task_exec_launch",
            "task_exec_review",
            "agent_job_start",
            "agent_schedule_wake",
            "agent_resume_status",
            "agent_resume_cancel",
        ] {
            assert!(
                !names.iter().any(|name| name == absent),
                "a worker must not be offered {absent}: {names:?}"
            );
        }
        for present in ["read_file", "write_file", "task_exec_deliver", "task_get"] {
            assert!(
                names.iter().any(|name| name == present),
                "a worker still needs {present}: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn worker_delivery_ignores_a_forged_valid_execution_id_and_uses_its_dispatch() {
        use crate::agents::tools::ToolCall;
        use crate::db::agent_dispatch::NewAgentDispatchJob;
        use crate::models::{AgentType, MessageTargetKind, OrchestrationActor, PlanningActorKind};

        let state = test_state();
        let (execution_a, execution_b) = state
            .db
            .with_conn(|conn| {
                let now = "2026-08-24T00:00:00Z";
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('principal-ab', 'Principal', ?1, ?1),
                            ('worker-a', 'Worker A', ?1, ?1),
                            ('worker-b', 'Worker B', ?1, ?1)",
                    [now],
                )?;
                conn.execute(
                    "INSERT INTO planning_tasks
                     (id, task_number, title, created_at, updated_at)
                     VALUES ('task-a', 901, 'Task A', ?1, ?1),
                            ('task-b', 902, 'Task B', ?1, ?1)",
                    [now],
                )?;
                let actor = OrchestrationActor {
                    kind: PlanningActorKind::Backend,
                    id: Some("test-orchestrator".into()),
                    session_id: None,
                    source_message_id: None,
                };
                let worker_agent = AgentType::Ollama;
                let mut execution_ids = Vec::new();
                for suffix in ["a", "b"] {
                    let task_id = format!("task-{suffix}");
                    let child_id = format!("worker-{suffix}");
                    let trigger_id = format!("trigger-{suffix}");
                    let dispatch_id = format!("dispatch-{suffix}");
                    let mut input =
                        crate::models::LaunchSingleTaskInput::new(&task_id, "principal-ab");
                    input.worker_target_kind = Some(MessageTargetKind::Agent);
                    input.worker_agent_type = Some(crate::db::orchestration::agent_type_to_db(
                        &AgentType::Ollama,
                    ));
                    let execution =
                        crate::db::orchestration::launch_single_task(conn, &input, &actor)?
                            .execution;
                    crate::db::orchestration::set_execution_sub_discussion(
                        conn,
                        &execution.id,
                        &child_id,
                    )?;
                    conn.execute(
                        "INSERT INTO messages
                         (id, discussion_id, role, content, timestamp, sort_order)
                         VALUES (?1, ?2, 'User', 'bounded task', ?3, 1)",
                        rusqlite::params![trigger_id, child_id, now],
                    )?;
                    crate::db::agent_dispatch::enqueue(
                        conn,
                        NewAgentDispatchJob {
                            id: &dispatch_id,
                            discussion_id: &child_id,
                            trigger_message_id: &trigger_id,
                            trigger_sort_order: 1,
                            dedupe_key: &dispatch_id,
                            agent_override: Some(&worker_agent),
                            chain_prompt_ids: &[],
                            batch_item: None,
                            group_id: None,
                            group_concurrency_limit: None,
                        },
                    )?;
                    crate::db::orchestration::attach_execution_dispatch(
                        conn,
                        &execution.id,
                        &dispatch_id,
                    )?;
                    // This fixture targets authorization before manifest parsing;
                    // provisioning normally performs the guarded transitions.
                    conn.execute(
                        "UPDATE task_executions SET status = 'Working' WHERE id = ?1",
                        [&execution.id],
                    )?;
                    execution_ids.push(execution.id);
                }
                Ok((execution_ids.remove(0), execution_ids.remove(0)))
            })
            .await
            .expect("seed two concurrent native worker executions");

        let worker = crate::api::agent_tools::KronnToolExecutor::arc_for_worker_room(
            state.clone(),
            Some("worker-a".into()),
            AgentType::Ollama,
            Some("trigger-a".into()),
            Some("dispatch-a".into()),
            None,
        );
        let outcome = worker
            .execute(&ToolCall {
                id: "deliver-a".into(),
                name: "task_exec_deliver".into(),
                arguments: serde_json::json!({
                    // B is a real concurrent execution. A worker-supplied id
                    // must have no influence over the server-derived target A.
                    "task_execution_id": execution_b,
                    "manifest": {}
                }),
            })
            .await;
        assert!(!outcome.ok);
        assert!(
            outcome.content["error"]
                .as_str()
                .is_some_and(|error| error.contains("DeliveryManifest v1 invalide")),
            "authorized execution A must reach manifest validation; a forged B would fail opaquely: {:?}",
            outcome.content
        );

        let principal = crate::api::agent_tools::KronnToolExecutor::arc(
            state,
            Some("principal-ab".into()),
            AgentType::Ollama,
            Some("principal-trigger".into()),
            Some("principal-dispatch".into()),
        );
        let missing_id = principal
            .execute(&ToolCall {
                id: "principal-deliver".into(),
                name: "task_exec_deliver".into(),
                arguments: serde_json::json!({"manifest": {}}),
            })
            .await;
        assert_eq!(
            missing_id.content["error"],
            "missing required field `task_execution_id`"
        );
        assert_ne!(execution_a, execution_b);
    }

    #[tokio::test]
    async fn an_ordinary_discussion_keeps_the_planning_tools() {
        // The narrowing is scoped to worker rooms. An HTTP agent helping with
        // the plan in a normal discussion legitimately reads and shapes the
        // backlog; taking those away would trade one bug for a worse one.
        let state = test_state();
        let discussion_id = "disc-ordinary-room";
        state
            .db
            .with_conn(move |conn| {
                let now = chrono::Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES (?1, 'Ordinary', ?2, ?2)",
                    rusqlite::params![discussion_id, now],
                )?;
                Ok(())
            })
            .await
            .expect("seed ordinary room");

        let agent = crate::api::agent_tools::KronnToolExecutor::arc(
            state.clone(),
            Some(discussion_id.into()),
            crate::models::AgentType::Ollama,
            None,
            None,
        );
        let names: Vec<String> = agent
            .catalogue()
            .iter()
            .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_string))
            .collect();

        for present in ["task_list", "plan_get", "task_exec_launch"] {
            assert!(
                names.iter().any(|name| name == present),
                "an ordinary discussion keeps {present}: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn tool_executor_resolves_the_config_id_belonging_to_the_slug() {
        use crate::api::agent_tools::KronnToolExecutor;
        use crate::models::{HostSyncMode, McpConfig};

        let state = test_state();
        // Two plugins wired, so picking "the first config" would be wrong.
        for (server_id, name) in [("mcp-resend", "Resend"), ("api-speedcurve", "SpeedCurve")] {
            let srv = crate::models::McpServer {
                id: server_id.into(),
                name: name.into(),
                description: String::new(),
                transport: crate::models::McpTransport::Stdio {
                    command: "true".into(),
                    args: vec![],
                },
                source: crate::models::McpSource::Manual,
                api_spec: None,
            };
            state
                .db
                .with_conn(move |conn| crate::db::mcps::upsert_server(conn, &srv))
                .await
                .expect("seed server");
        }
        for (id, server_id, label) in [
            ("cfg-resend", "mcp-resend", "resend"),
            ("cfg-speedcurve", "api-speedcurve", "SpeedCurve"),
        ] {
            let cfg = McpConfig {
                id: id.into(),
                server_id: server_id.into(),
                label: label.into(),
                env_keys: vec![],
                env_encrypted: String::new(),
                args_override: None,
                is_global: true,
                include_general: true,
                config_hash: format!("hash-{id}"),
                project_ids: vec![],
                host_sync: HostSyncMode::None,
            };
            state
                .db
                .with_conn(move |conn| crate::db::mcps::insert_config(conn, &cfg))
                .await
                .expect("seed config");
        }

        let exec = KronnToolExecutor::new(state, None);
        assert_eq!(
            exec.resolve_config_id_pub("api-speedcurve")
                .await
                .as_deref(),
            Some("cfg-speedcurve"),
            "resolved another plugin's config — this is the bug that broke api_call"
        );
        assert_eq!(
            exec.resolve_config_id_pub("mcp-resend").await.as_deref(),
            Some("cfg-resend")
        );
        // An unwired plugin has no config: that is a real answer, and api_call
        // reports it rather than silently calling something else.
        assert_eq!(exec.resolve_config_id_pub("api-ghost").await, None);
    }

    #[tokio::test]
    async fn agent_resume_jobs_are_readable_and_cancellable_by_discussion() {
        let state = test_state();
        state
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO discussions
                     (id, title, agent, language, created_at, updated_at)
                     VALUES ('disc-resume-api', 'Durable resume', 'Ollama', 'fr', 'now', 'now')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO agent_resume_jobs (
                         id, discussion_id, target_agent_json, kind, status, dedupe_key,
                         reason, scheduled_at, created_at, updated_at
                     ) VALUES ('resume-api-1', 'disc-resume-api', '\"Ollama\"',
                               'Wake', 'Pending', 'resume:api:one', 'waiting for CI',
                               '2099-01-01T00:00:00Z', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let live_token = tokio_util::sync::CancellationToken::new();
        state
            .cancel_registry
            .lock()
            .unwrap()
            .insert("agent-job:resume-api-1".into(), live_token.clone());

        let request = Request::builder()
            .uri("/api/discussions/disc-resume-api/agent-resume-jobs")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert_eq!(json["data"][0]["id"], "resume-api-1");
        assert_eq!(json["data"][0]["status"], "pending");
        assert_eq!(json["data"][0]["reason"], "waiting for CI");

        let request = Request::builder()
            .method("POST")
            .uri("/api/discussions/disc-resume-api/agent-resume-jobs/resume-api-1/cancel")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["success"], true);
        assert_eq!(json["data"]["status"], "cancelled");
        assert!(live_token.is_cancelled());
        assert!(!state
            .cancel_registry
            .lock()
            .unwrap()
            .contains_key("agent-job:resume-api-1"));

        state
            .db
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO agent_resume_jobs (
                         id, discussion_id, target_agent_json, kind, status, dedupe_key,
                         reason, scheduled_at, created_at, updated_at
                     ) VALUES ('resume-api-2', 'disc-resume-api', '\"Ollama\"',
                               'Wake', 'Pending', 'resume:api:two', 'wait for deploy',
                               '2099-01-01T00:00:00Z', 'now', 'now')",
                    [],
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/api/discussions/disc-resume-api/stop")
            .body(Body::empty())
            .unwrap();
        let (status, json) = send(state.clone(), false, request).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"]["cancelled"], true);
        let stopped = state
            .db
            .with_conn(|connection| crate::db::agent_jobs::get(connection, "resume-api-2"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stopped.view.status,
            crate::models::AgentResumeJobStatus::Cancelled
        );
    }
}
