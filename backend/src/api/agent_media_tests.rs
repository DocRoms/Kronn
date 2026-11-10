//! KT-1605 — a workflow Agent step on an HTTP agent generates media as a
//! discussion agent does.
use super::super::*;
use super::{read_capped, too_large};

fn connection(
    id: &str,
    alias: &str,
    video_model: Option<&str>,
) -> crate::models::ExternalApiConnection {
    crate::models::ExternalApiConnection {
        id: id.into(),
        display_name: alias.into(),
        mention_alias: alias.into(),
        // Closed on purpose: nothing here may reach a provider.
        endpoint: Some("http://127.0.0.1:1".into()),
        credential_slug: alias.into(),
        origin_preset: crate::models::ExternalApiConnectionPreset::OpenRouter,
        economy_model: None,
        default_model: None,
        reasoning_model: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        image_model: None,
        video_model: video_model.map(str::to_string),
        decision_model: None,
        tools_enabled: None,
        media_endpoint: None,
    }
}

/// A project, a workflow, a run and the given connections.
async fn state_with_run(connections: Vec<crate::models::ExternalApiConnection>) -> AppState {
    let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('p','p','p','2026-11-01T00:00:00Z','2026-11-01T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflows(id, name, project_id, trigger_json, steps_json, created_at, updated_at) \
             VALUES ('wf', 'wf', 'p', '{\"type\":\"Manual\"}', '[]', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
            [],
        )?;
        conn.execute(
            "INSERT INTO workflow_runs(id, workflow_id, project_id, started_at) VALUES ('media-run', 'wf', 'p', '2026-11-01T00:00:00Z')",
            [],
        )?;
        for connection in &connections {
            crate::db::external_api_connections::insert(conn, connection)?;
        }
        Ok(())
    })
    .await
    .unwrap();
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        )),
        db,
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

fn step(state: &AppState) -> std::sync::Arc<dyn ToolExecutor> {
    KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("p".into()),
        "media-run".into(),
        "clip".into(),
    )
}

async fn run(executor: &dyn ToolExecutor, name: &str, arguments: Value) -> ToolOutcome {
    executor
        .execute(&ToolCall {
            id: format!("call-{name}"),
            name: name.into(),
            arguments,
        })
        .await
}

fn clip(prompt: &str) -> Value {
    json!({"modality": "video", "prompt": prompt})
}

#[tokio::test]
async fn a_workflow_step_is_offered_the_media_tools_and_no_other_orchestration_tool() {
    let state = state_with_run(Vec::new()).await;
    let catalogue = step(&state).catalogue();
    let names: Vec<&str> = catalogue
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"media_generate") && names.contains(&"media_job_status"));
    for refused in [
        "agent_list",
        "task_exec_prepare",
        "task_exec_launch",
        "task_exec_deliver",
        "task_exec_review",
        "task_exec_cancel",
        "task_exec_reassign",
        "task_create",
        "task_update",
        "agent_job_start",
        "agent_schedule_wake",
    ] {
        assert!(!names.contains(&refused), "{refused} must stay refused");
    }
    let generate = catalogue
        .iter()
        .find(|tool| tool["function"]["name"] == "media_generate")
        .unwrap();
    // A step has no room to wake: the option is not offered.
    assert!(generate["function"]["parameters"]["properties"]
        .get("wake_when_ready")
        .is_none());
    assert!(generate["function"]["description"]
        .as_str()
        .unwrap()
        .contains("`wait_secs`"));
    let status = catalogue
        .iter()
        .find(|tool| tool["function"]["name"] == "media_job_status")
        .unwrap();
    assert_eq!(
        status["function"]["parameters"]["properties"]["wait_secs"]["type"],
        "integer"
    );
    // A discussion agent keeps its own declarations untouched.
    let discussion = orchestration_tool_catalogue();
    let discussion_generate = discussion
        .iter()
        .find(|tool| tool["function"]["name"] == "media_generate")
        .unwrap();
    assert!(discussion_generate["function"]["parameters"]["properties"]
        .get("wake_when_ready")
        .is_some());
}

#[tokio::test]
async fn every_other_orchestration_tool_is_refused_when_a_step_names_it() {
    let state = state_with_run(Vec::new()).await;
    let executor = step(&state);
    for name in [
        "agent_list",
        "task_exec_launch",
        "task_exec_review",
        "agent_job_start",
    ] {
        let outcome = run(executor.as_ref(), name, json!({})).await;
        assert!(!outcome.ok, "{name} ran in a workflow step");
        assert!(
            outcome.content["error"]
                .as_str()
                .unwrap()
                .contains("not allowed in workflow Agent steps"),
            "{name}: {}",
            outcome.content
        );
    }
}

#[tokio::test]
async fn a_step_generates_in_a_room_of_its_run_and_reuses_it() {
    let state = state_with_run(vec![connection(
        "conn-1",
        "openrouter",
        Some("google/veo-3.1-lite"),
    )])
    .await;
    let executor = step(&state);

    let first = run(
        executor.as_ref(),
        "media_generate",
        clip("a lighthouse at dusk"),
    )
    .await;
    assert!(first.ok, "{}", first.content);
    // The job itself, not the HTTP envelope around it.
    assert_eq!(first.content["model"], "google/veo-3.1-lite");
    assert_eq!(first.content["status"], "pending");
    assert_eq!(first.content["connection_id"], "conn-1");
    // Opened by the step before the job, so the references it brings and the
    // job share it.
    assert_eq!(first.content["discussion_created"], false);
    let room = first.content["discussion_id"].as_str().unwrap().to_string();

    let (linked, project): (Option<String>, Option<String>) = state
        .db
        .with_read_conn({
            let room = room.clone();
            move |conn| {
                Ok(conn.query_row(
                    "SELECT workflow_run_id, project_id FROM discussions WHERE id = ?1",
                    [&room],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(linked.as_deref(), Some("media-run"));
    assert_eq!(project.as_deref(), Some("p"));

    // Another take from the same step lands in the same room.
    let second = run(
        executor.as_ref(),
        "media_generate",
        clip("the same lighthouse at dawn"),
    )
    .await;
    assert!(second.ok, "{}", second.content);
    assert_eq!(second.content["discussion_id"], json!(room));
    assert_eq!(second.content["discussion_created"], false);
    assert_ne!(second.content["job_id"], first.content["job_id"]);

    // And a retry of the very same call buys nothing a second time.
    let retry = run(
        executor.as_ref(),
        "media_generate",
        clip("a lighthouse at dusk"),
    )
    .await;
    assert!(retry.ok, "{}", retry.content);
    assert_eq!(retry.content["job_id"], first.content["job_id"]);
}

#[tokio::test]
async fn the_discussion_validation_applies_in_a_step() {
    let state = state_with_run(vec![connection(
        "conn-1",
        "openrouter",
        Some("google/veo-3.1-lite"),
    )])
    .await;
    let executor = step(&state);

    // The modality is required, never defaulted.
    let none = run(executor.as_ref(), "media_generate", json!({"prompt": "x"})).await;
    assert!(!none.ok);
    assert!(none.content["error"]
        .as_str()
        .unwrap()
        .contains("`modality`"));

    // A modality with no configured model is refused with the way out.
    let image = run(
        executor.as_ref(),
        "media_generate",
        json!({"modality": "image", "prompt": "x"}),
    )
    .await;
    assert!(!image.ok);
    assert!(image.content["error"]
        .as_str()
        .unwrap()
        .contains("No connection has a image model configured"));

    // A resolution outside the allowed ones is refused before anything is queued.
    let resolution = run(
        executor.as_ref(),
        "media_generate",
        json!({"modality": "video", "prompt": "x", "resolution": "9000p"}),
    )
    .await;
    assert!(!resolution.ok);

    // There is no room to wake: refused before anything is billed.
    let wake = run(
        executor.as_ref(),
        "media_generate",
        json!({"modality": "video", "prompt": "x", "wake_when_ready": true}),
    )
    .await;
    assert!(!wake.ok);
    assert!(wake.content["error"]
        .as_str()
        .unwrap()
        .contains("wait_secs"));
    let queued: i64 = state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM media_jobs", [], |row| row.get(0))?)
        })
        .await
        .unwrap();
    assert_eq!(queued, 0, "a refused call must not queue a job");
}

#[tokio::test]
async fn several_connections_are_named_rather_than_chosen_in_a_step() {
    let state = state_with_run(vec![
        connection("conn-1", "openrouter", Some("google/veo-3.1-lite")),
        connection("conn-2", "litellm", Some("bytedance/seedance-2.0-mini")),
    ])
    .await;
    let executor = step(&state);
    let refused = run(executor.as_ref(), "media_generate", clip("x")).await;
    assert!(!refused.ok);
    let error = refused.content["error"].as_str().unwrap();
    assert!(
        error.contains("conn-1") && error.contains("conn-2"),
        "{error}"
    );

    let chosen = run(
        executor.as_ref(),
        "media_generate",
        json!({"modality": "video", "prompt": "x", "connection_id": "litellm"}),
    )
    .await;
    assert!(chosen.ok, "{}", chosen.content);
    assert_eq!(chosen.content["connection_id"], "conn-2");
}

#[tokio::test]
async fn the_billing_guard_stops_a_step_after_a_failed_generation_and_caps_them() {
    let state = state_with_run(vec![connection(
        "conn-1",
        "openrouter",
        Some("google/veo-3.1-lite"),
    )])
    .await;
    let executor = step(&state);

    let first = run(executor.as_ref(), "media_generate", clip("take 0")).await;
    assert!(first.ok, "{}", first.content);
    let job = first.content["job_id"].as_str().unwrap().to_string();
    state
        .db
        .with_conn(move |conn| {
            crate::db::media_jobs::fail(
                conn,
                &job,
                crate::models::MediaJobStatus::Failed,
                "provider refused",
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();

    // Retrying, with another parameter or not, is the loop the guard ends.
    let retried = run(
        executor.as_ref(),
        "media_generate",
        json!({"modality": "video", "prompt": "take 0", "duration_secs": 3}),
    )
    .await;
    assert!(!retried.ok);
    assert!(
        retried.content["error"]
            .as_str()
            .unwrap()
            .contains("did not complete"),
        "{}",
        retried.content
    );

    // Another step of the same run is another request.
    let other = KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("p".into()),
        "media-run".into(),
        "second-clip".into(),
    );
    let fresh = run(other.as_ref(), "media_generate", clip("take 0")).await;
    assert!(fresh.ok, "{}", fresh.content);
}

#[tokio::test]
async fn a_step_waits_for_the_job_and_finds_the_asset_in_the_run_artifacts() {
    let state = state_with_run(vec![connection(
        "conn-1",
        "openrouter",
        Some("google/veo-3.1-lite"),
    )])
    .await;
    let executor = step(&state);
    let started = run(executor.as_ref(), "media_generate", clip("a lighthouse")).await;
    assert!(started.ok, "{}", started.content);
    let job = started.content["job_id"].as_str().unwrap().to_string();
    let room = started.content["discussion_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Not settled yet: said so, and told not to generate again.
    let pending = run(
        executor.as_ref(),
        "media_job_status",
        json!({"job_id": job}),
    )
    .await;
    assert!(pending.ok, "{}", pending.content);
    assert_eq!(pending.content["settled"], false);
    assert!(pending.content["note"]
        .as_str()
        .unwrap()
        .contains("Do not start the generation again"));

    // The media worker finishes while the step is waiting.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("clip.mp4");
    std::fs::write(&source, b"not really a video").unwrap();
    let finisher = {
        let (state, job, room) = (state.clone(), job.clone(), room.clone());
        let source = source.to_string_lossy().into_owned();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            state
                .db
                .with_conn(move |conn| {
                    crate::db::discussions::insert_context_file(
                        conn,
                        "asset-1",
                        &room,
                        "clip.mp4",
                        "video/mp4",
                        18,
                        "",
                        Some(&source),
                    )?;
                    crate::db::media_jobs::complete(
                        conn,
                        &job,
                        crate::db::media_jobs::Completion {
                            context_file_id: "asset-1",
                            rendered: &Default::default(),
                            cost: None,
                            generation_id: None,
                        },
                        chrono::Utc::now(),
                    )
                })
                .await
                .unwrap();
        })
    };

    let waited = run(
        executor.as_ref(),
        "media_job_status",
        json!({"job_id": job, "wait_secs": 30}),
    )
    .await;
    finisher.await.unwrap();
    assert!(waited.ok, "{}", waited.content);
    assert_eq!(waited.content["status"], "completed");
    assert_eq!(waited.content["settled"], true);
    assert_eq!(waited.content["asset"]["context_file_id"], "asset-1");
    assert_eq!(waited.content["asset"]["discussion_id"], json!(room));
    let artifact = waited.content["asset"]["artifact_path"]
        .as_str()
        .unwrap_or_else(|| panic!("no artifact_path: {}", waited.content));
    assert_eq!(std::fs::read(artifact).unwrap(), b"not really a video");
    // Compared as real paths: the temp directory may be a symlink.
    let artifacts = crate::workflows::run_artifacts::dir_for("media-run")
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(
        std::fs::canonicalize(artifact)
            .unwrap()
            .starts_with(&artifacts),
        "{artifact} is outside {}",
        artifacts.display()
    );
    assert!(artifact.ends_with(".mp4"));
    crate::workflows::run_artifacts::remove("media-run");
}

#[tokio::test]
async fn a_step_cannot_read_a_job_it_did_not_start() {
    let state = state_with_run(vec![connection(
        "conn-1",
        "openrouter",
        Some("google/veo-3.1-lite"),
    )])
    .await;
    // A job of an unrelated project's room.
    state
        .db
        .with_conn(|conn| {
            conn.execute(
                "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES ('other','o','o','2026-11-01T00:00:00Z','2026-11-01T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO discussions (id,title,project_id,created_at,updated_at) VALUES ('other-room','r','other','2026-11-01T00:00:00Z','2026-11-01T00:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO media_jobs (id, modality, status, connection_id, model, prompt, params_json, discussion_id, project_id, is_byok, attempts, scheduled_at, deadline_at, created_at, updated_at) \
                 VALUES ('foreign-job', 'video', 'pending', 'conn-1', 'm', 'p', '{}', 'other-room', 'other', 0, 0, '2026-11-01T00:00:00Z', '2026-11-01T01:00:00Z', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let outcome = run(
        step(&state).as_ref(),
        "media_job_status",
        json!({"job_id": "foreign-job"}),
    )
    .await;
    assert!(!outcome.ok, "{}", outcome.content);
}

// ── KT-1605 review: scopes, caps, references ─────────────────────────────

async fn add_project(state: &AppState, id: &str, path: &str) {
    let (id, path) = (id.to_string(), path.to_string());
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id,name,path,created_at,updated_at) VALUES (?1,?1,?2,'2026-11-01T00:00:00Z','2026-11-01T00:00:00Z')",
                [&id, &path],
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

/// A room of `project` holding one context file backed by a real file.
async fn add_room_with_file(
    state: &AppState,
    project: &str,
    room: &str,
    file: &str,
    filename: &str,
    mime: &str,
    path: &std::path::Path,
) {
    let (project, room, file, filename, mime, path) = (
        project.to_string(),
        room.to_string(),
        file.to_string(),
        filename.to_string(),
        mime.to_string(),
        path.to_string_lossy().into_owned(),
    );
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id,title,project_id,created_at,updated_at) VALUES (?1,'r',?2,'2026-11-01T00:00:00Z','2026-11-01T00:00:00Z')",
                [&room, &project],
            )?;
            crate::db::discussions::insert_context_file(
                conn, &file, &room, &filename, &mime, 4, "", Some(&path),
            )?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn queued(state: &AppState) -> i64 {
    state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM media_jobs", [], |row| row.get(0))?)
        })
        .await
        .unwrap()
}

async fn media_rooms(state: &AppState) -> i64 {
    state
        .db
        .with_read_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM discussions WHERE title LIKE 'Media · %'",
                [],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap()
}

fn error_of(outcome: &ToolOutcome) -> String {
    outcome.content["error"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn one_connection() -> Vec<crate::models::ExternalApiConnection> {
    let mut both = connection("conn-1", "openrouter", Some("google/veo-3.1-lite"));
    both.image_model = Some("black-forest-labs/flux-schnell".into());
    vec![both]
}

fn in_step(state: &AppState, name: &str) -> std::sync::Arc<dyn ToolExecutor> {
    KronnToolExecutor::workflow_arc(
        state.clone(),
        Some("p".into()),
        "media-run".into(),
        name.into(),
    )
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[test]
fn a_link_planted_at_the_asset_name_leaves_its_target_untouched() {
    let run = "media-run-planted-link";
    let directory = crate::workflows::run_artifacts::ensure(run).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let victim = outside.path().join("victim");
    std::fs::write(&victim, b"keep").unwrap();
    // An Exec step of the run can write in its artifacts directory.
    std::os::unix::fs::symlink(&victim, directory.join("job-1.mp4")).unwrap();
    let source = outside.path().join("clip.mp4");
    std::fs::write(&source, b"the asset").unwrap();

    let copied = super::copy_into_run(run, "job-1", "clip.mp4", &source.to_string_lossy()).unwrap();

    assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
    assert!(!std::fs::symlink_metadata(&copied).unwrap().is_symlink());
    assert_eq!(std::fs::read(&copied).unwrap(), b"the asset");
    crate::workflows::run_artifacts::remove(run);
}

#[tokio::test]
async fn test_presses_of_a_same_named_step_share_neither_a_room_nor_a_cap() {
    let state = state_with_run(one_connection()).await;
    add_project(&state, "q", "q").await;
    let press = |project: &str| {
        KronnToolExecutor::workflow_test_arc(
            state.clone(),
            Some(project.into()),
            "clip".into(),
            false,
        )
    };
    let (in_p, in_q, again_in_p) = (press("p"), press("q"), press("p"));
    let first = run(in_p.as_ref(), "media_generate", clip("take 0")).await;
    let other_project = run(in_q.as_ref(), "media_generate", clip("take 0")).await;
    let other_press = run(again_in_p.as_ref(), "media_generate", clip("take 0")).await;
    for outcome in [&first, &other_project, &other_press] {
        assert!(outcome.ok, "{}", outcome.content);
    }
    let rooms: std::collections::HashSet<_> = [&first, &other_project, &other_press]
        .iter()
        .map(|outcome| {
            outcome.content["discussion_id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(rooms.len(), 3, "a room is shared between presses");
    let q_room = other_project.content["discussion_id"]
        .as_str()
        .unwrap()
        .to_string();
    let q_project: Option<String> = state
        .db
        .with_read_conn(move |conn| {
            Ok(conn.query_row(
                "SELECT project_id FROM discussions WHERE id = ?1",
                [&q_room],
                |row| row.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(q_project.as_deref(), Some("q"));

    // One press whose generation failed blocks that press only.
    let job = first.content["job_id"].as_str().unwrap().to_string();
    state
        .db
        .with_conn(move |conn| {
            crate::db::media_jobs::fail(
                conn,
                &job,
                crate::models::MediaJobStatus::Failed,
                "provider refused",
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();
    let blocked = run(in_p.as_ref(), "media_generate", clip("take 1")).await;
    assert!(
        error_of(&blocked).contains("did not complete"),
        "{blocked:?}"
    );
    assert!(
        run(in_q.as_ref(), "media_generate", clip("take 1"))
            .await
            .ok
    );
    assert!(
        run(again_in_p.as_ref(), "media_generate", clip("take 1"))
            .await
            .ok
    );

    // Filling one press's ceiling leaves the others' alone.
    for take in 2..crate::api::media_request_guard::MAX_JOBS_PER_REQUEST {
        let outcome = run(
            again_in_p.as_ref(),
            "media_generate",
            clip(&format!("take {take}")),
        )
        .await;
        assert!(outcome.ok, "{}", outcome.content);
    }
    let full = run(again_in_p.as_ref(), "media_generate", clip("one more")).await;
    assert!(error_of(&full).contains("already started"), "{full:?}");
    assert!(
        run(in_q.as_ref(), "media_generate", clip("take 2"))
            .await
            .ok
    );
}

#[tokio::test]
async fn a_dry_run_step_starts_no_generation_and_opens_no_room() {
    let state = state_with_run(one_connection()).await;
    let dry =
        KronnToolExecutor::workflow_test_arc(state.clone(), Some("p".into()), "clip".into(), true);
    let outcome = run(dry.as_ref(), "media_generate", clip("a lighthouse")).await;
    assert!(!outcome.ok);
    assert!(error_of(&outcome).contains("dry run"), "{outcome:?}");
    assert_eq!(queued(&state).await, 0);
    assert_eq!(media_rooms(&state).await, 0);
}

#[tokio::test]
async fn a_step_stops_at_the_request_ceiling_and_a_run_at_its_total() {
    let state = state_with_run(one_connection()).await;
    let ceiling = crate::api::media_request_guard::MAX_JOBS_PER_REQUEST;
    let first = in_step(&state, "a");
    for take in 0..ceiling {
        let outcome = run(
            first.as_ref(),
            "media_generate",
            clip(&format!("take {take}")),
        )
        .await;
        assert!(outcome.ok, "take {take}: {}", outcome.content);
    }
    let over = run(first.as_ref(), "media_generate", clip("one more")).await;
    assert!(!over.ok);
    assert!(
        error_of(&over).contains(&format!("already started {ceiling} generations")),
        "{over:?}"
    );

    // Other steps are other requests, up to the run's total.
    let second = in_step(&state, "b");
    for take in 0..ceiling {
        let outcome = run(
            second.as_ref(),
            "media_generate",
            clip(&format!("take {take}")),
        )
        .await;
        assert!(outcome.ok, "take {take}: {}", outcome.content);
    }
    assert_eq!(queued(&state).await, 2 * ceiling as i64);
    assert_eq!(super::DEFAULT_RUN_CAP, 2 * ceiling);
    let third = run(
        in_step(&state, "c").as_ref(),
        "media_generate",
        clip("take 0"),
    )
    .await;
    assert!(!third.ok);
    assert!(
        error_of(&third).contains("workflow run already started"),
        "{third:?}"
    );
    assert_eq!(queued(&state).await, 2 * ceiling as i64);
    assert_eq!(media_rooms(&state).await, 2, "a refusal left a room behind");
}

#[tokio::test]
async fn calls_launched_together_open_one_room_and_cannot_pass_the_cap() {
    let state = state_with_run(one_connection()).await;
    let executor = step(&state);
    let calls: Vec<_> = (0..10)
        .map(|take| {
            let executor = executor.clone();
            tokio::spawn(async move {
                run(
                    executor.as_ref(),
                    "media_generate",
                    clip(&format!("take {take}")),
                )
                .await
            })
        })
        .collect();
    let mut accepted = Vec::new();
    for call in calls {
        let outcome = call.await.unwrap();
        if outcome.ok {
            accepted.push(
                outcome.content["discussion_id"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
    }
    let ceiling = crate::api::media_request_guard::MAX_JOBS_PER_REQUEST;
    assert_eq!(accepted.len(), ceiling);
    accepted.dedup();
    assert_eq!(accepted.len(), 1, "two rooms for one step");
    assert_eq!(media_rooms(&state).await, 1);
    assert_eq!(queued(&state).await as usize, ceiling);
}

/// A picture file, in a room of `project`, ready to be named as a reference.
async fn brand_image(state: &AppState, project: &str, room: &str, file: &str) {
    let directory = tempfile::tempdir().unwrap().keep();
    let path = directory.join("brand.png");
    std::fs::write(&path, PNG).unwrap();
    add_room_with_file(state, project, room, file, "brand.png", "image/png", &path).await;
}

fn first_frame(prompt: &str, reference: &str) -> Value {
    json!({
        "modality": "video",
        "prompt": prompt,
        "reference_asset_ids": [reference],
        "reference_mode": "first_frame",
    })
}

/// The context file ids a queued job was given, and the room it is in.
async fn references_of(state: &AppState, job: &str) -> (Vec<String>, String) {
    let job = job.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            let job = crate::db::media_jobs::get(conn, &job)?.unwrap();
            Ok((job.params.reference_asset_ids, job.discussion_id.unwrap()))
        })
        .await
        .unwrap()
}

async fn file_of(state: &AppState, id: &str) -> crate::models::ContextFile {
    let id = id.to_string();
    state
        .db
        .with_read_conn(move |conn| {
            Ok(crate::db::discussions::get_context_file(conn, &id)?.unwrap())
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn a_brand_image_named_by_context_file_id_is_usable_and_a_foreign_one_is_not() {
    let state = state_with_run(one_connection()).await;
    add_project(&state, "other", "other").await;
    brand_image(&state, "p", "brand-room", "brand-1").await;
    brand_image(&state, "other", "foreign-room", "foreign-1").await;
    let executor = step(&state);

    let used = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "brand-1"),
    )
    .await;
    assert!(used.ok, "{}", used.content);
    let (references, room) = references_of(&state, used.content["job_id"].as_str().unwrap()).await;
    assert_eq!(references.len(), 1);
    let copy = file_of(&state, &references[0]).await;
    // Resolved inside the step's own room, never the project's.
    assert_eq!(copy.discussion_id, room);
    assert_ne!(copy.id, "brand-1");
    assert_eq!(std::fs::read(copy.disk_path.unwrap()).unwrap(), PNG);

    // The same picture named again is the same copy.
    let again = run(
        executor.as_ref(),
        "media_generate",
        first_frame("another clip", "brand-1"),
    )
    .await;
    assert!(again.ok, "{}", again.content);
    let (references_again, _) =
        references_of(&state, again.content["job_id"].as_str().unwrap()).await;
    assert_eq!(references_again, references);

    // Another project's file is out of scope, and says nothing about it.
    let second = in_step(&state, "second");
    let foreign = run(
        second.as_ref(),
        "media_generate",
        first_frame("a clip", "foreign-1"),
    )
    .await;
    assert!(!foreign.ok);
    assert!(
        error_of(&foreign).contains("not in this workflow run's scope"),
        "{foreign:?}"
    );
    let missing = run(
        second.as_ref(),
        "media_generate",
        first_frame("a clip", "no-such-id"),
    )
    .await;
    assert!(!missing.ok);
    assert_eq!(media_rooms(&state).await, 1, "a refusal left a room behind");
    assert_eq!(queued(&state).await, 2);
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[tokio::test]
async fn a_project_file_is_a_reference_and_cannot_leave_the_project() {
    let state = state_with_run(one_connection()).await;
    let project = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("brand")).unwrap();
    std::fs::write(project.path().join("brand/logo.png"), PNG).unwrap();
    std::fs::write(project.path().join("notes.txt"), b"text").unwrap();
    std::fs::write(outside.path().join("secret.png"), PNG).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.png"),
        project.path().join("brand/linked.png"),
    )
    .unwrap();
    let path = project.path().to_string_lossy().into_owned();
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'p'", [&path])?;
            Ok(())
        })
        .await
        .unwrap();
    let executor = step(&state);

    let used = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "brand/logo.png"),
    )
    .await;
    assert!(used.ok, "{}", used.content);
    let (references, room) = references_of(&state, used.content["job_id"].as_str().unwrap()).await;
    let copy = file_of(&state, &references[0]).await;
    assert_eq!(copy.discussion_id, room);
    assert_eq!(copy.mime_type, "image/png");
    assert_eq!(std::fs::read(copy.disk_path.unwrap()).unwrap(), PNG);

    for (reference, said) in [
        ("../secret.png", "no `..`"),
        ("brand/linked.png", "a link on its path is refused"),
        ("notes.txt", "png, jpg, webp or gif"),
    ] {
        let refused = run(
            executor.as_ref(),
            "media_generate",
            first_frame("other", reference),
        )
        .await;
        assert!(!refused.ok, "{reference} was accepted");
        assert!(
            error_of(&refused).contains(said),
            "{reference}: {refused:?}"
        );
    }
    let absent = run(
        executor.as_ref(),
        "media_generate",
        first_frame("other", "brand/absent.png"),
    )
    .await;
    assert!(
        error_of(&absent).contains("not in this workflow run's scope"),
        "{absent:?}"
    );
    let escape = outside
        .path()
        .join("secret.png")
        .to_string_lossy()
        .into_owned();
    let absolute = run(
        executor.as_ref(),
        "media_generate",
        first_frame("other", &escape),
    )
    .await;
    assert!(!absolute.ok, "{absolute:?}");
    assert_eq!(queued(&state).await, 1);
}

/// The first bytes of a PNG: what a picture is judged by, never its name.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nbody";

/// A project at a fresh directory, for the project-file tests.
async fn project_at(state: &AppState) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().to_string_lossy().into_owned();
    state
        .db
        .with_conn(move |conn| {
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'p'", [&path])?;
            Ok(())
        })
        .await
        .unwrap();
    project
}

#[cfg(unix)] // asserts POSIX mode bits, which Windows does not have; uses a POSIX FIFO
#[tokio::test]
async fn a_fifo_named_like_a_picture_is_refused_without_blocking() {
    let state = state_with_run(one_connection()).await;
    let project = project_at(&state).await;
    let fifo =
        std::ffi::CString::new(project.path().join("x.png").to_string_lossy().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let executor = step(&state);

    let refused = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        run(
            executor.as_ref(),
            "media_generate",
            first_frame("a clip", "x.png"),
        ),
    )
    .await
    .expect("a FIFO held the open (and the run lock) forever");
    assert!(!refused.ok, "{refused:?}");
    assert!(
        error_of(&refused).contains("regular picture file"),
        "{refused:?}"
    );
    assert_eq!(queued(&state).await, 0);
    assert_eq!(media_rooms(&state).await, 0, "a refusal left a room behind");
}

#[cfg(unix)] // creates symlinks through std::os::unix
#[tokio::test]
async fn a_directory_swapped_for_a_link_is_refused_not_followed() {
    let state = state_with_run(one_connection()).await;
    let project = project_at(&state).await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), PNG).unwrap();
    // `brand` was a directory of the project when the path was checked.
    std::os::unix::fs::symlink(outside.path(), project.path().join("brand")).unwrap();
    // The leaf itself, planted as a link to a picture inside the project.
    std::fs::write(project.path().join("real.png"), PNG).unwrap();
    std::os::unix::fs::symlink("real.png", project.path().join("alias.png")).unwrap();
    let executor = step(&state);

    for reference in ["brand/secret.png", "alias.png"] {
        let refused = run(
            executor.as_ref(),
            "media_generate",
            first_frame("a clip", reference),
        )
        .await;
        assert!(!refused.ok, "{reference} was followed");
        assert!(
            error_of(&refused).contains("a link on its path is refused"),
            "{reference}: {refused:?}"
        );
    }
    assert_eq!(queued(&state).await, 0);
}

#[tokio::test]
async fn a_path_through_a_file_is_not_a_directory_not_a_link() {
    let state = state_with_run(one_connection()).await;
    let project = project_at(&state).await;
    std::fs::write(project.path().join("a.png"), PNG).unwrap();
    let executor = step(&state);

    let refused = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "a.png/x"),
    )
    .await;
    assert!(!refused.ok, "{refused:?}");
    let message = error_of(&refused);
    assert!(message.contains("not a directory"), "{message}");
    assert!(!message.contains("link"), "{message}");
    assert_eq!(queued(&state).await, 0);
    assert_eq!(media_rooms(&state).await, 0, "a refusal left a room behind");
}

#[tokio::test]
async fn a_project_file_over_the_cap_is_refused() {
    let state = state_with_run(one_connection()).await;
    let project = project_at(&state).await;
    // Sparse: it takes no disk, and starts as a PNG.
    let big = std::fs::File::create(project.path().join("big.png")).unwrap();
    big.set_len(crate::api::media::MAX_REFERENCE_BYTES + 1)
        .unwrap();
    std::fs::write(project.path().join("small.png"), PNG).unwrap();
    let executor = step(&state);

    let refused = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "big.png"),
    )
    .await;
    assert!(!refused.ok, "{refused:?}");
    assert_eq!(queued(&state).await, 0);

    // A file that grew after the size check: the read itself hits the cap.
    let grown = std::io::Read::take(
        std::io::repeat(0),
        crate::api::media::MAX_REFERENCE_BYTES + 5,
    );
    let error = read_capped(grown, 10).unwrap_err();
    assert_eq!(
        error,
        too_large(crate::api::media::MAX_REFERENCE_BYTES + 1),
        "a file that grew past the cap was accepted"
    );
    let exact = std::io::Read::take(std::io::repeat(0), crate::api::media::MAX_REFERENCE_BYTES);
    assert_eq!(
        read_capped(exact, 10).unwrap().len() as u64,
        crate::api::media::MAX_REFERENCE_BYTES
    );
}

#[tokio::test]
async fn a_project_file_is_a_picture_by_its_first_bytes_not_its_name() {
    let state = state_with_run(one_connection()).await;
    let project = project_at(&state).await;
    std::fs::write(project.path().join("fake.png"), b"not a picture at all").unwrap();
    // A JPEG under a .png name is a picture, stored as what it is.
    std::fs::write(
        project.path().join("really-jpeg.png"),
        [
            0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F', 0, 1,
        ],
    )
    .unwrap();
    // A picture under a name that says nothing.
    std::fs::write(project.path().join("logo"), PNG).unwrap();
    let executor = step(&state);

    let fake = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "fake.png"),
    )
    .await;
    assert!(!fake.ok, "{fake:?}");
    assert!(
        error_of(&fake).contains("png, jpg, webp or gif"),
        "{fake:?}"
    );

    let jpeg = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", "really-jpeg.png"),
    )
    .await;
    assert!(jpeg.ok, "{}", jpeg.content);
    let (references, _) = references_of(&state, jpeg.content["job_id"].as_str().unwrap()).await;
    assert_eq!(
        file_of(&state, &references[0]).await.mime_type,
        "image/jpeg"
    );

    let bare = run(
        executor.as_ref(),
        "media_generate",
        first_frame("another clip", "logo"),
    )
    .await;
    assert!(bare.ok, "{}", bare.content);
}

#[tokio::test]
async fn a_name_that_matches_nothing_gets_the_one_scope_refusal() {
    let state = state_with_run(one_connection()).await;
    let _project = project_at(&state).await;
    let executor = step(&state);
    for reference in ["no-such-id", "brand/absent.png", "./nothing", "a/b/c/d.png"] {
        let refused = run(
            executor.as_ref(),
            "media_generate",
            first_frame("a clip", reference),
        )
        .await;
        assert!(!refused.ok, "{reference} was accepted");
        assert!(
            error_of(&refused).contains("not in this workflow run's scope"),
            "{reference}: {refused:?}"
        );
    }
    assert_eq!(queued(&state).await, 0);
    assert_eq!(media_rooms(&state).await, 0);
}

#[test]
fn the_run_cap_is_bounded_and_falls_back_to_its_default() {
    assert_eq!(super::bounded_run_cap(None), super::DEFAULT_RUN_CAP);
    assert_eq!(super::bounded_run_cap(Some("0")), super::DEFAULT_RUN_CAP);
    assert_eq!(super::bounded_run_cap(Some("lots")), super::DEFAULT_RUN_CAP);
    assert_eq!(super::bounded_run_cap(Some(" 30 ")), 30);
    assert_eq!(super::bounded_run_cap(Some("1000000")), super::MAX_RUN_CAP);
}

/// Finishes `job` with a real picture in its room.
async fn complete_with_picture(state: &AppState, job: &str, file: &str) {
    let (_, room) = references_of(state, job).await;
    let directory = tempfile::tempdir().unwrap().keep();
    let path = directory.join("image.png");
    std::fs::write(&path, PNG).unwrap();
    let (job, file, path) = (
        job.to_string(),
        file.to_string(),
        path.to_string_lossy().into_owned(),
    );
    state
        .db
        .with_conn(move |conn| {
            crate::db::discussions::insert_context_file(
                conn,
                &file,
                &room,
                "image.png",
                "image/png",
                PNG.len() as u64,
                "",
                Some(&path),
            )?;
            crate::db::media_jobs::complete(
                conn,
                &job,
                crate::db::media_jobs::Completion {
                    context_file_id: &file,
                    rendered: &Default::default(),
                    cost: None,
                    generation_id: None,
                },
                chrono::Utc::now(),
            )
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn an_earlier_step_s_picture_is_a_reference_by_job_id_or_artifact_path() {
    let state = state_with_run(one_connection()).await;
    let image_step = in_step(&state, "image");
    let started = run(
        image_step.as_ref(),
        "media_generate",
        json!({"modality": "image", "prompt": "a lighthouse"}),
    )
    .await;
    assert!(started.ok, "{}", started.content);
    let job = started.content["job_id"].as_str().unwrap().to_string();

    // Not finished: said so, nothing is queued.
    let early = run(
        in_step(&state, "clip").as_ref(),
        "media_generate",
        first_frame("a clip", &job),
    )
    .await;
    assert!(!early.ok);
    assert!(
        error_of(&early).contains("wait for it with media_job_status"),
        "{early:?}"
    );

    complete_with_picture(&state, &job, "made-1").await;
    let by_job = run(
        in_step(&state, "clip").as_ref(),
        "media_generate",
        first_frame("a clip", &job),
    )
    .await;
    assert!(by_job.ok, "{}", by_job.content);
    let (references, room) =
        references_of(&state, by_job.content["job_id"].as_str().unwrap()).await;
    let copy = file_of(&state, &references[0]).await;
    assert_eq!(
        copy.discussion_id, room,
        "the picture is brought into the clip step's room"
    );
    assert_ne!(copy.id, "made-1");
    assert_eq!(std::fs::read(copy.disk_path.unwrap()).unwrap(), PNG);

    // The artifact path the status tool returned names the same picture.
    let status = run(
        image_step.as_ref(),
        "media_job_status",
        json!({"job_id": job}),
    )
    .await;
    let artifact = status.content["asset"]["artifact_path"]
        .as_str()
        .unwrap()
        .to_string();
    let by_path = run(
        in_step(&state, "clip-2").as_ref(),
        "media_generate",
        first_frame("a clip", &artifact),
    )
    .await;
    assert!(by_path.ok, "{}", by_path.content);
    let (references, _) = references_of(&state, by_path.content["job_id"].as_str().unwrap()).await;
    assert_eq!(
        std::fs::read(file_of(&state, &references[0]).await.disk_path.unwrap()).unwrap(),
        PNG
    );
    crate::workflows::run_artifacts::remove("media-run");
}

#[tokio::test]
async fn another_run_s_or_project_s_generation_is_not_a_reference() {
    let state = state_with_run(one_connection()).await;
    add_project(&state, "other", "other").await;
    // A finished picture of another run of the same project, one of a human's
    // own generation in the project, and one of another project's run.
    for (job, file, project, room, root) in [
        (
            "sibling-job",
            "sibling-file",
            "p",
            "sibling-room",
            Some("workflow-step:Workflow sibling-run · image"),
        ),
        ("human-job", "human-file", "p", "human-room", None),
        (
            "foreign-job",
            "foreign-file",
            "other",
            "foreign-room",
            Some("workflow-step:Workflow foreign-run · image"),
        ),
    ] {
        brand_image(&state, project, room, file).await;
        let (job, file, project, room) = (
            job.to_string(),
            file.to_string(),
            project.to_string(),
            room.to_string(),
        );
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO media_jobs (id, modality, status, connection_id, model, prompt, params_json, discussion_id, project_id, request_root_id, context_file_id, is_byok, attempts, scheduled_at, deadline_at, created_at, updated_at) \
                     VALUES (?1, 'image', 'completed', 'conn-1', 'm', 'p', '{}', ?2, ?3, ?4, ?5, 0, 0, '2026-11-01T00:00:00Z', '2026-11-01T01:00:00Z', '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
                    rusqlite::params![job, room, project, root, file],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }
    let executor = step(&state);
    for job in ["sibling-job", "human-job", "foreign-job"] {
        let refused = run(
            executor.as_ref(),
            "media_generate",
            first_frame("a clip", job),
        )
        .await;
        assert!(!refused.ok, "{job} was accepted");
        assert!(
            error_of(&refused).contains("not in this workflow run's scope"),
            "{job}: {refused:?}"
        );
    }
    // Nor is a path in another run's artifacts directory.
    let elsewhere = crate::workflows::run_artifacts::ensure("sibling-run")
        .unwrap()
        .join("sibling-job.png");
    let by_path = run(
        executor.as_ref(),
        "media_generate",
        first_frame("a clip", &elsewhere.to_string_lossy()),
    )
    .await;
    assert!(!by_path.ok, "{by_path:?}");
    assert_eq!(queued(&state).await, 3, "only the fixtures");
    assert_eq!(media_rooms(&state).await, 0);
    crate::workflows::run_artifacts::remove("sibling-run");
}
