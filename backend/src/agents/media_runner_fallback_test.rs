// KT-1706 — the runner honours the media fallback: a generation the provider
// refused BEFORE accepting it moves to the fallback of its mode; one it may be
// producing never does.

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use tokio::sync::RwLock;

use super::fall_back_if_refused;
use crate::core::comfyui::CONNECTION_ID as COMFYUI;
use crate::core::connection_fallback::set_row;
use crate::db::media_jobs::{self, MediaJob, NewMediaJob};
use crate::models::{
    ApiKey, ConnectionFallbackTarget, ExternalApiConnection, ExternalApiConnectionPreset,
    FallbackMode, MediaJobStatus, MediaModality, MediaParams,
};

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-12T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn to(connection: &str) -> ConnectionFallbackTarget {
    ConnectionFallbackTarget {
        connection: connection.into(),
        model: None,
    }
}

fn connection(id: &str, image: Option<&str>, video: Option<&str>) -> ExternalApiConnection {
    ExternalApiConnection {
        id: id.into(),
        display_name: id.into(),
        mention_alias: id.into(),
        endpoint: Some("https://media.example.test".into()),
        credential_slug: format!("slug-{id}"),
        origin_preset: ExternalApiConnectionPreset::OpenRouter,
        economy_model: None,
        default_model: None,
        reasoning_model: None,
        created_at: now(),
        updated_at: now(),
        image_model: image.map(str::to_string),
        video_model: video.map(str::to_string),
        media_endpoint: None,
        decision_model: None,
        tools_enabled: None,
    }
}

async fn state() -> crate::AppState {
    let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory db"));
    let config = Arc::new(RwLock::new(crate::core::config::default_config()));
    crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
}

/// `conn-b` is usable (endpoint, key, image and video slots); `conn-c` has no key.
async fn with_connections(state: &crate::AppState) {
    state
        .db
        .with_conn(|conn| {
            for c in [
                connection("conn-a", Some("a-img"), Some("a-vid")),
                connection("conn-b", Some("b-img"), Some("b-vid")),
                connection("conn-c", Some("c-img"), Some("c-vid")),
            ] {
                crate::db::external_api_connections::insert(conn, &c)?;
            }
            Ok(())
        })
        .await
        .expect("seed connections");
    let mut config = state.config.write().await;
    for id in ["conn-a", "conn-b"] {
        config.tokens.keys.push(ApiKey {
            id: id.into(),
            name: id.into(),
            provider: format!("slug-{id}"),
            value: "secret".into(),
            active: true,
        });
    }
}

async fn seed(
    state: &crate::AppState,
    connection_id: &str,
    modality: MediaModality,
    params: MediaParams,
    handle: Option<&str>,
) -> MediaJob {
    let (id, connection_id, handle) = (
        "job-1".to_string(),
        connection_id.to_string(),
        handle.map(str::to_string),
    );
    state
        .db
        .with_conn(move |conn| {
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: &id,
                    modality,
                    connection_id: &connection_id,
                    model: "first-model",
                    prompt: "un chat",
                    params: &params,
                    discussion_id: None,
                    message_id: None,
                    project_id: None,
                    scheduled_at: now(),
                    deadline_at: now() + Duration::minutes(20),
                },
                now(),
            )?;
            media_jobs::claim(conn, &id, now())?;
            media_jobs::mark_submit_attempt(conn, &id, now())?;
            if let Some(handle) = handle.as_deref() {
                media_jobs::record_submission(conn, &id, handle, now())?;
            }
            Ok(())
        })
        .await
        .expect("seed job");
    stored(state).await
}

async fn stored(state: &crate::AppState) -> MediaJob {
    state
        .db
        .with_read_conn(|conn| media_jobs::get(conn, "job-1"))
        .await
        .expect("read")
        .expect("job")
}

async fn set_fallback(
    state: &crate::AppState,
    owner: &str,
    mode: FallbackMode,
    target: ConnectionFallbackTarget,
) {
    set_row(
        &mut state.config.write().await.agents,
        owner,
        mode,
        Some(target),
    );
}

const RATE_LIMIT: &str = "provider answered HTTP 429: rate limit exceeded, too many requests";

#[tokio::test]
async fn a_refused_generation_moves_to_the_fallback_of_its_mode() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, "conn-a", FallbackMode::Video, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Video,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(job.submit_attempted_at.is_some());

    let moved = fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap();
    assert!(moved);

    let job = stored(&state).await;
    assert_eq!(job.connection_id, "conn-b");
    // The model is the target's own video slot, never the one of the source.
    assert_eq!(job.model, "b-vid");
    assert_eq!(job.status, MediaJobStatus::Pending);
    // A fresh submission: the old mark must not make the new source refuse it.
    assert!(job.submit_attempted_at.is_none());
    assert_eq!(job.attempts, 0);
    assert_eq!(job.params.fallback_from, vec!["conn-a".to_string()]);
    assert!(job.last_error.as_deref().unwrap().contains("conn-a"));
}

#[tokio::test]
async fn an_explicit_model_on_the_row_wins_over_the_slot_of_the_target() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(
        &state,
        "conn-a",
        FallbackMode::Image,
        ConnectionFallbackTarget {
            connection: "conn-b".into(),
            model: Some("chosen-img".into()),
        },
    )
    .await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Image,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    assert_eq!(stored(&state).await.model, "chosen-img");
}

#[tokio::test]
async fn only_the_row_of_the_kind_of_the_job_applies() {
    let state = state().await;
    with_connections(&state).await;
    // An image fallback does not take a video job.
    set_fallback(&state, "conn-a", FallbackMode::Image, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Video,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(!fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    assert_eq!(stored(&state).await.connection_id, "conn-a");
}

#[tokio::test]
async fn a_failure_the_provider_may_have_accepted_never_falls_back() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, "conn-a", FallbackMode::Video, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Video,
        MediaParams::default(),
        None,
    )
    .await;
    // An outage or a lost connection after the request left: it may be generating.
    for reason in [
        "provider answered HTTP 503 service unavailable",
        "error sending request: operation timed out",
        "the generation failed: content policy",
    ] {
        assert!(
            !fall_back_if_refused(&state, &job, Some(reason), now())
                .await
                .unwrap(),
            "{reason}"
        );
    }
    assert_eq!(stored(&state).await.connection_id, "conn-a");
}

#[tokio::test]
async fn a_job_the_provider_has_a_handle_for_is_never_moved() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, "conn-a", FallbackMode::Video, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Video,
        MediaParams::default(),
        Some("handle-1"),
    )
    .await;
    assert!(!fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    let job = stored(&state).await;
    assert_eq!(job.connection_id, "conn-a");
    assert_eq!(job.provider_job_id.as_deref(), Some("handle-1"));
}

#[tokio::test]
async fn a_job_with_a_source_image_is_not_moved() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, "conn-a", FallbackMode::Video, to("conn-b")).await;
    let params = MediaParams {
        reference_asset_ids: vec!["file-1".into()],
        ..Default::default()
    };
    let job = seed(&state, "conn-a", MediaModality::Video, params, None).await;
    assert!(!fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
}

#[tokio::test]
async fn a_target_that_cannot_serve_is_skipped_and_the_walk_goes_on() {
    let state = state().await;
    with_connections(&state).await;
    // conn-c has no key: skipped; conn-c's own row leads to conn-b.
    set_fallback(&state, "conn-a", FallbackMode::Image, to("conn-c")).await;
    set_fallback(&state, "conn-c", FallbackMode::Image, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Image,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    let job = stored(&state).await;
    assert_eq!(job.connection_id, "conn-b");
    assert_eq!(
        job.params.fallback_from,
        vec!["conn-a".to_string(), "conn-c".to_string()]
    );
}

#[tokio::test]
async fn a_source_the_job_already_left_is_never_offered_again() {
    let state = state().await;
    with_connections(&state).await;
    // A loop written by hand: a → b → a.
    set_fallback(&state, "conn-a", FallbackMode::Image, to("conn-b")).await;
    set_fallback(&state, "conn-b", FallbackMode::Image, to("conn-a")).await;
    let params = MediaParams {
        fallback_from: vec!["conn-a".into()],
        ..Default::default()
    };
    let job = seed(&state, "conn-b", MediaModality::Image, params, None).await;
    assert!(!fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    assert_eq!(stored(&state).await.connection_id, "conn-b");
}

#[tokio::test]
async fn a_stopped_comfyui_hands_its_image_to_the_fallback() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, COMFYUI, FallbackMode::Image, to("conn-b")).await;
    let job = seed(
        &state,
        COMFYUI,
        MediaModality::Image,
        MediaParams::default(),
        None,
    )
    .await;
    let reason =
        "ComfyUI does not answer at http://127.0.0.1:8188: start it, then retry the generation";
    assert!(fall_back_if_refused(&state, &job, Some(reason), now())
        .await
        .unwrap());
    let job = stored(&state).await;
    assert_eq!(job.connection_id, "conn-b");
    assert_eq!(job.model, "b-img");
    assert_eq!(job.status, MediaJobStatus::Pending);
}

#[tokio::test]
async fn a_remote_source_falls_back_to_comfyui_with_its_own_slot() {
    let state = state().await;
    with_connections(&state).await;
    state.config.write().await.comfyui.video_model = Some("wan.safetensors".into());
    set_fallback(&state, "conn-a", FallbackMode::Video, to(COMFYUI)).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Video,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
    let job = stored(&state).await;
    assert_eq!(job.connection_id, COMFYUI);
    assert_eq!(job.model, "wan.safetensors");
}

#[tokio::test]
async fn a_job_the_advance_itself_settled_as_failed_is_read_from_the_store() {
    let state = state().await;
    with_connections(&state).await;
    set_fallback(&state, "conn-a", FallbackMode::Image, to("conn-b")).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Image,
        MediaParams::default(),
        None,
    )
    .await;
    state
        .db
        .with_conn(|conn| {
            media_jobs::fail(conn, "job-1", MediaJobStatus::Failed, RATE_LIMIT, now())?;
            Ok(())
        })
        .await
        .unwrap();
    // No error from the advance (it settled the job itself): the stored one counts.
    assert!(fall_back_if_refused(&state, &job, None, now())
        .await
        .unwrap());
    let job = stored(&state).await;
    assert_eq!(job.status, MediaJobStatus::Pending);
    assert_eq!(job.connection_id, "conn-b");
    // A cancelled or completed job is left alone.
    state
        .db
        .with_conn(|conn| {
            media_jobs::fail(conn, "job-1", MediaJobStatus::Cancelled, RATE_LIMIT, now())?;
            Ok(())
        })
        .await
        .unwrap();
    assert!(!fall_back_if_refused(&state, &job, None, now())
        .await
        .unwrap());
}

#[tokio::test]
async fn without_a_row_nothing_moves() {
    let state = state().await;
    with_connections(&state).await;
    let job = seed(
        &state,
        "conn-a",
        MediaModality::Image,
        MediaParams::default(),
        None,
    )
    .await;
    assert!(!fall_back_if_refused(&state, &job, Some(RATE_LIMIT), now())
        .await
        .unwrap());
}
