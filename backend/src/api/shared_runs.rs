use crate::{
    models::{ApiResponse, MediaJobStatus, SharedRun},
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ListRunsQuery {
    pub kind: Option<String>,
    pub source_id: Option<String>,
    pub project_id: Option<String>,
    pub discussion_id: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: u32,
}
fn default_limit() -> u32 {
    50
}

pub async fn persist_and_broadcast(state: &AppState, run: SharedRun) -> anyhow::Result<()> {
    let saved = run.clone();
    state
        .db
        .with_conn(move |conn| crate::db::shared_runs::upsert(conn, &saved))
        .await?;
    let _ = state
        .ws_broadcast
        .send(crate::models::WsMessage::SharedRunUpdated { run_id: run.id });
    Ok(())
}

/// Single publication point for a media job: read the STORED job, project it,
/// persist and broadcast. Every durable transition goes through here, so a
/// caller can never persist a run without emitting the event a live view needs
/// — the two used to be separable, and the broadcast was simply forgotten.
pub async fn publish_media_job(state: &AppState, job_id: &str) -> anyhow::Result<()> {
    let lookup = job_id.to_string();
    let job = state
        .db
        .with_read_conn(move |conn| crate::db::media_jobs::get(conn, &lookup))
        .await?;
    let Some(job) = job else {
        // The job vanished (cascade delete): nothing to publish, and inventing
        // a run for it would leave a ghost in every live view.
        return Ok(());
    };
    let run = crate::db::shared_runs::media_run(&job);
    let published = persist_and_broadcast(state, run).await;

    // A finished generation added a file to the discussion. Without this the
    // assets tab and the message attachments only pick it up on a manual
    // reload, which for a 100 s video reads as "nothing happened".
    if let (MediaJobStatus::Completed, Some(discussion_id), Some(message_id)) = (
        job.status,
        job.discussion_id.as_deref(),
        job.message_id.as_deref(),
    ) {
        if job.context_file_id.is_some() {
            let _ = state
                .ws_broadcast
                .send(crate::models::WsMessage::ContextFilesChanged {
                    discussion_id: discussion_id.to_string(),
                    message_id: message_id.to_string(),
                });
        }
    }
    published
}

/// Scoping the run listing by project/discussion is only meaningful when the
/// caller's scope actually exists: an unknown id silently returning an empty
/// list would look identical to "this project/discussion has no runs yet",
/// which hides typos and stale ids from a rehydrating client. Fail closed
/// with an explicit error instead.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListRunsQuery>,
) -> Json<ApiResponse<Vec<SharedRun>>> {
    if let Some(project_id) = query.project_id.clone() {
        match state
            .db
            .with_conn(move |conn| crate::db::projects::get_project(conn, &project_id))
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Json(ApiResponse::err("Unknown project_id scope")),
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    }
    if let Some(discussion_id) = query.discussion_id.clone() {
        match state
            .db
            .with_conn(move |conn| crate::db::discussions::get_discussion(conn, &discussion_id))
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return Json(ApiResponse::err("Unknown discussion_id scope")),
            Err(error) => return Json(ApiResponse::err(format!("DB error: {error}"))),
        }
    }
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::shared_runs::list(
                conn,
                query.kind.as_deref(),
                query.source_id.as_deref(),
                query.project_id.as_deref(),
                query.discussion_id.as_deref(),
                query.limit,
            )
        })
        .await;
    match result {
        Ok(runs) => Json(ApiResponse::ok(runs)),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<SharedRun>> {
    match state
        .db
        .with_conn(move |conn| crate::db::shared_runs::get(conn, &id))
        .await
    {
        Ok(Some(run)) => Json(ApiResponse::ok(run)),
        Ok(None) => Json(ApiResponse::err("Run not found")),
        Err(error) => Json(ApiResponse::err(format!("DB error: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::media_jobs::{self, Completion, NewMediaJob};
    use crate::models::{MediaModality, MediaParams, MediaRendered, WsMessage};
    use crate::DEFAULT_MAX_CONCURRENT_AGENTS;
    use chrono::Utc;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    #[tokio::test]
    async fn completing_a_media_job_tells_the_open_discussion_its_files_changed() {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory db"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS);

        let now = Utc::now();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at) \
                     VALUES ('d-1', 'Media', ?1, ?1)",
                    ["2026-09-01T09:00:00Z"],
                )?;
                conn.execute(
                    "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) \
                     VALUES ('m-launch', 'd-1', 'User', 'un chat', ?1, 1)",
                    ["2026-09-01T09:00:00Z"],
                )?;
                crate::db::discussions::insert_context_file(
                    conn,
                    "file-1",
                    "d-1",
                    "image-abc.png",
                    "image/png",
                    1234,
                    "",
                    Some("/tmp/kronn-test/image-abc.png"),
                )?;
                media_jobs::insert(
                    conn,
                    NewMediaJob {
                        id: "job-1",
                        modality: MediaModality::Image,
                        connection_id: "conn-1",
                        model: "google/gemini-2.5-flash-image",
                        prompt: "un chat",
                        params: &MediaParams::default(),
                        discussion_id: Some("d-1"),
                        message_id: Some("m-launch"),
                        project_id: None,
                        scheduled_at: now,
                        deadline_at: now + chrono::Duration::minutes(20),
                    },
                    now,
                )?;
                media_jobs::complete(
                    conn,
                    "job-1",
                    Completion {
                        context_file_id: "file-1",
                        rendered: &MediaRendered::default(),
                        cost: None,
                        generation_id: None,
                    },
                    now,
                )?;
                Ok(())
            })
            .await
            .expect("seed");

        // Subscribed before publishing: the broadcast is not replayed, so a
        // late subscriber would see nothing and the test would prove nothing.
        let mut ws = state.ws_broadcast.subscribe();
        publish_media_job(&state, "job-1")
            .await
            .expect("publication");

        let mut refreshed = false;
        while let Ok(message) = ws.try_recv() {
            if let WsMessage::ContextFilesChanged {
                discussion_id,
                message_id,
            } = message
            {
                if discussion_id == "d-1" && message_id == "m-launch" {
                    refreshed = true;
                }
            }
        }
        assert!(
            refreshed,
            "a finished generation must announce the new file, or the assets tab \
             only shows it after a manual reload"
        );
    }

    #[tokio::test]
    async fn a_job_still_running_announces_no_file() {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory db"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        let state = AppState::new_defaults(config, db, DEFAULT_MAX_CONCURRENT_AGENTS);
        let now = Utc::now();
        state
            .db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at) \
                     VALUES ('d-1', 'Media', ?1, ?1)",
                    ["2026-09-01T09:00:00Z"],
                )?;
                media_jobs::insert(
                    conn,
                    NewMediaJob {
                        id: "job-2",
                        modality: MediaModality::Video,
                        connection_id: "conn-1",
                        model: "bytedance/seedance-2.0-mini",
                        prompt: "un chat",
                        params: &MediaParams::default(),
                        discussion_id: Some("d-1"),
                        message_id: None,
                        project_id: None,
                        scheduled_at: now,
                        deadline_at: now + chrono::Duration::minutes(20),
                    },
                    now,
                )?;
                Ok(())
            })
            .await
            .expect("seed");

        let mut ws = state.ws_broadcast.subscribe();
        publish_media_job(&state, "job-2")
            .await
            .expect("publication");
        while let Ok(message) = ws.try_recv() {
            assert!(
                !matches!(message, WsMessage::ContextFilesChanged { .. }),
                "nothing was attached yet; announcing a file change would make \
                 the UI refetch for nothing"
            );
        }
    }
}
