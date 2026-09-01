//! Media generation endpoints.
//!
//! Deliberately available before the composer UI exists: it is what makes an
//! end-to-end test possible, and it is the same surface the UI and the MCP
//! tool will call.
//!
//! A request never carries a model name. The model comes from the connection's
//! configured slot, so a caller cannot dispatch an arbitrary — and arbitrarily
//! priced — model, and a missing slot is refused with a message that says what
//! to configure.

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::Utc;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agents::media_worker::DEFAULT_DEADLINE;
use crate::db::media_jobs::{self, NewMediaJob};
use crate::models::{ApiResponse, MediaJobStatus, MediaModality, MediaParams};
use crate::AppState;

/// Hard ceilings for any caller. The human UI shows an estimate before sending;
/// these stop a mistake — or an agent — from ordering an expensive generation.
const MAX_DURATION_SECS: u32 = 15;
const ALLOWED_RESOLUTIONS: [&str; 3] = ["480p", "720p", "1080p"];

#[derive(Debug, Deserialize)]
pub struct GenerateMediaRequest {
    /// Stable client-generated key for this launch intention. Retrying the
    /// same request with the same key returns the already-created durable job
    /// instead of scheduling a second billable provider call.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    pub connection_id: String,
    pub modality: MediaModality,
    pub prompt: String,
    /// Discussion the asset gets attached to. When absent, one is created with
    /// the prompt as its first message: the asset is a context file and a
    /// context file belongs to a discussion, so there must always be one — and
    /// an agent asking for a standalone image should not have to invent a room
    /// first.
    #[serde(default)]
    pub discussion_id: Option<String>,
    /// Existing message that triggered the generation. It is a source link,
    /// never the asset anchor: every launch gets its own transcript message.
    /// A source requires `discussion_id` and must belong to that discussion.
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub duration_secs: Option<u32>,
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub aspect_ratio: Option<String>,
    #[serde(default)]
    pub generate_audio: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct GenerateMediaResponse {
    pub job_id: String,
    pub status: MediaJobStatus,
    /// Model resolved from the connection, echoed so the caller can see what
    /// will actually be billed.
    pub model: String,
    /// Discussion the asset will land in — including the one this call just
    /// created. Without it a caller that passed no discussion cannot tell
    /// where its own generation went.
    pub discussion_id: String,
    /// Fresh message dedicated to this launch. The placeholder and eventual
    /// asset both render at this durable transcript slot.
    pub message_id: String,
}

#[derive(Debug, Serialize)]
pub struct MediaJobView {
    pub id: String,
    pub modality: MediaModality,
    pub status: MediaJobStatus,
    pub model: String,
    pub discussion_id: Option<String>,
    /// Message the finished asset hangs from.
    pub message_id: Option<String>,
    pub context_file_id: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub is_byok: Option<bool>,
    pub last_error: Option<String>,
    pub attempts: u32,
}

pub async fn generate(
    State(state): State<AppState>,
    Json(req): Json<GenerateMediaRequest>,
) -> Json<ApiResponse<GenerateMediaResponse>> {
    let prompt = req.prompt.trim().to_string();
    if prompt.is_empty() {
        return Json(ApiResponse::err("prompt is required".to_string()));
    }

    if let Some(duration) = req.duration_secs {
        if duration == 0 || duration > MAX_DURATION_SECS {
            return Json(ApiResponse::err(format!(
                "duration must be between 1 and {MAX_DURATION_SECS} seconds"
            )));
        }
    }
    if let Some(resolution) = req.resolution.as_deref() {
        if !ALLOWED_RESOLUTIONS.contains(&resolution) {
            return Json(ApiResponse::err(format!(
                "resolution must be one of {}",
                ALLOWED_RESOLUTIONS.join(", ")
            )));
        }
    }

    let lookup = req.connection_id.clone();
    let connection = match state
        .db
        .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup))
        .await
    {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            return Json(ApiResponse::err(format!(
                "unknown connection: {}",
                req.connection_id
            )))
        }
        Err(e) => return Json(ApiResponse::err(format!("failed to read connection: {e}"))),
    };

    // The slot, not the request, decides the model.
    let model = match req.modality {
        MediaModality::Image => connection.image_model.clone(),
        MediaModality::Video => connection.video_model.clone(),
    };
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
        return Json(ApiResponse::err(format!(
            "connection '{}' has no {} model configured",
            connection.display_name,
            req.modality.as_str()
        )));
    };

    // Every non-DB validation has passed. From here on, the discussion (when
    // one has to be created), its anchor message and the job itself are
    // written in ONE transaction: a rejected request or a mid-write DB failure
    // must never leave an orphan prompt message with no job behind it
    // (KT-549 blocker #1).
    let idempotency_key = req.idempotency_key.as_deref().map(str::trim);
    if idempotency_key
        .map(|key| !(8..=200).contains(&key.len()))
        .unwrap_or(false)
    {
        return Json(ApiResponse::err(
            "idempotency_key must contain between 8 and 200 characters".to_string(),
        ));
    }

    let now = Utc::now();
    let deadline =
        now + chrono::Duration::from_std(DEFAULT_DEADLINE).unwrap_or(chrono::Duration::minutes(20));
    let job_id = idempotency_key
        .map(|key| idempotent_job_id(&connection.id, key))
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let params = MediaParams {
        duration_secs: req.duration_secs,
        resolution: req.resolution.clone(),
        aspect_ratio: req.aspect_ratio.clone(),
        generate_audio: req.generate_audio,
    };
    if idempotency_key.is_some() {
        let existing_lookup = job_id.clone();
        match state
            .db
            .with_read_conn(move |conn| media_jobs::get(conn, &existing_lookup))
            .await
        {
            Ok(Some(existing)) => {
                return Json(idempotent_response(
                    &existing,
                    &connection.id,
                    req.modality,
                    &model,
                    &prompt,
                    &params,
                    req.discussion_id.as_deref(),
                ));
            }
            Ok(None) => {}
            Err(e) => {
                return Json(ApiResponse::err(format!(
                    "failed to check idempotency key: {e}"
                )))
            }
        }
    }
    let launch = MediaLaunchWrite {
        job_id: job_id.clone(),
        modality: req.modality,
        connection_id: connection.id.clone(),
        model: model.clone(),
        prompt: prompt.clone(),
        params,
        source_message_id: req.message_id.clone(),
        scheduled_at: now,
        deadline_at: deadline,
    };
    let (discussion_id, anchor) = match state
        .db
        .with_conn(move |conn| write_media_launch(conn, req.discussion_id, launch))
        .await
    {
        Ok(result) => result,
        Err(e) => {
            // Two identical requests can race after both observed no row. The
            // deterministic primary key lets only one transaction commit; the
            // loser returns that winner instead of surfacing an error that
            // would invite another retry.
            let retry_lookup = job_id.clone();
            match state
                .db
                .with_read_conn(move |conn| media_jobs::get(conn, &retry_lookup))
                .await
            {
                Ok(Some(existing)) => {
                    return Json(idempotent_response(
                        &existing,
                        &connection.id,
                        req.modality,
                        &model,
                        &prompt,
                        &params,
                        req.discussion_id.as_deref(),
                    ));
                }
                _ => return Json(ApiResponse::err(format!("failed to queue job: {e}"))),
            }
        }
    };

    // Published through the single point, so the run is visible AND broadcast
    // while the job is still queued — a 100 s generation must not be invisible
    // until the provider answers. If this particular publish fails, nothing is
    // lost: the job already committed as `pending`, due immediately, so the
    // next worker sweep claims it and republishes through the same idempotent
    // upsert (`persist_and_broadcast` keys on the job id) — repairable, not a
    // second chance to double-insert.
    if let Err(e) = crate::api::shared_runs::publish_media_job(&state, &job_id).await {
        tracing::warn!(job = %job_id, error = %e, "media run publication failed");
    }

    Json(ApiResponse::ok(GenerateMediaResponse {
        job_id,
        status: MediaJobStatus::Pending,
        model,
        discussion_id,
        message_id: anchor,
    }))
}

fn idempotent_job_id(connection_id: &str, key: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"kronn-media-launch-v1\0");
    digest.update(connection_id.as_bytes());
    digest.update(b"\0");
    digest.update(key.as_bytes());
    format!("media-{:x}", digest.finalize())
}

fn idempotent_response(
    existing: &media_jobs::MediaJob,
    connection_id: &str,
    modality: MediaModality,
    model: &str,
    prompt: &str,
    params: &MediaParams,
    requested_discussion_id: Option<&str>,
) -> ApiResponse<GenerateMediaResponse> {
    let same_intention = existing.connection_id == connection_id
        && existing.modality == modality
        && existing.model == model
        && existing.prompt == prompt
        && existing.params == *params
        && requested_discussion_id
            .map(|id| existing.discussion_id.as_deref() == Some(id))
            .unwrap_or(true);
    if !same_intention {
        return ApiResponse::err(
            "idempotency_key was already used for a different media request".to_string(),
        );
    }
    let (Some(discussion_id), Some(message_id)) =
        (existing.discussion_id.clone(), existing.message_id.clone())
    else {
        return ApiResponse::err("idempotent media job has no discussion anchor".to_string());
    };
    ApiResponse::ok(GenerateMediaResponse {
        job_id: existing.id.clone(),
        status: existing.status,
        model: existing.model.clone(),
        discussion_id,
        message_id,
    })
}

/// Builds the message a media launch anchors to when it names no message of
/// its own: the prompt becomes its content, so the transcript slot explains
/// itself instead of showing a bare card with no context.
fn build_prompt_message(
    prompt: &str,
    now: chrono::DateTime<Utc>,
    reply_to_message_id: Option<String>,
    media_job_id: &str,
) -> (String, crate::models::DiscussionMessage) {
    use crate::models::{DiscussionMessage, MessageChannel, MessageRole};

    let message_id = uuid::Uuid::new_v4().to_string();
    let message = DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: message_id.clone(),
        role: MessageRole::User,
        channel: MessageChannel::Main,
        content: prompt.to_string(),
        agent_type: None,
        timestamp: now,
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: None,
        author_avatar_email: None,
        source_msg_id: Some(format!("kronn-media-anchor:{media_job_id}")),
        duration_ms: None,
        target_agent: None,
        reply_to_message_id,
    };
    (message_id, message)
}

/// Everything about a launch that was already resolved (connection, model)
/// before `generate` allows a single write.
struct MediaLaunchWrite {
    job_id: String,
    modality: MediaModality,
    connection_id: String,
    model: String,
    prompt: String,
    params: MediaParams,
    source_message_id: Option<String>,
    scheduled_at: chrono::DateTime<Utc>,
    deadline_at: chrono::DateTime<Utc>,
}

/// The write side of `generate`: creates (if needed) the discussion and its
/// anchor message, then the job — all in ONE transaction. A job insert
/// failure, or a crash in between, must never leave an orphan prompt message
/// with no job behind it (KT-549 blocker #1). A plain function, not an inline
/// closure, so a forced job-insert failure can be exercised directly in a
/// unit test without going through the HTTP layer's random job id.
fn write_media_launch(
    conn: &rusqlite::Connection,
    existing_discussion: Option<String>,
    launch: MediaLaunchWrite,
) -> anyhow::Result<(String, String)> {
    let transaction = conn.unchecked_transaction()?;

    let (discussion_id, project_id, anchor) = match existing_discussion {
        Some(discussion_id) => {
            let project_id = transaction
                .query_row(
                    "SELECT project_id FROM discussions WHERE id = ?1",
                    rusqlite::params![discussion_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("unknown discussion: {discussion_id}"))?;

            if let Some(source_id) = launch.source_message_id.as_deref() {
                let belongs: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1 AND discussion_id = ?2)",
                    rusqlite::params![source_id, discussion_id],
                    |row| row.get(0),
                )?;
                if !belongs {
                    anyhow::bail!("source message does not belong to discussion");
                }
            }

            // The supplied message is causal context, never the rendering
            // anchor. Every billable launch owns one fresh transcript slot so
            // two generations from the same source cannot overwrite each
            // other or hide the source message.
            let (anchor, message) = build_prompt_message(
                &launch.prompt,
                launch.scheduled_at,
                launch.source_message_id.clone(),
                &launch.job_id,
            );
            crate::db::discussions::insert_message(&transaction, &discussion_id, &message)?;
            (discussion_id, project_id, anchor)
        }
        None => {
            if launch.source_message_id.is_some() {
                anyhow::bail!("message_id requires discussion_id");
            }
            let (discussion_id, anchor) = insert_media_discussion(
                &transaction,
                &launch.prompt,
                launch.scheduled_at,
                &launch.job_id,
            )?;
            (discussion_id, None, anchor)
        }
    };

    media_jobs::insert(
        &transaction,
        NewMediaJob {
            id: &launch.job_id,
            modality: launch.modality,
            connection_id: &launch.connection_id,
            model: &launch.model,
            prompt: &launch.prompt,
            params: &launch.params,
            discussion_id: Some(&discussion_id),
            message_id: Some(&anchor),
            project_id: project_id.as_deref(),
            scheduled_at: launch.scheduled_at,
            deadline_at: launch.deadline_at,
        },
        launch.scheduled_at,
    )?;

    transaction.commit()?;
    Ok((discussion_id, anchor))
}

/// Creates the discussion an asset needs when the caller has none, and its
/// anchor message in the same breath.
///
/// The prompt becomes the first message, so the room explains itself: a
/// discussion holding a generated video with no trace of what was asked would
/// be unreadable a day later. Takes a plain `&Connection` deliberately: the
/// caller already opened the transaction this must land in, together with the
/// media job that follows it.
fn insert_media_discussion(
    conn: &rusqlite::Connection,
    prompt: &str,
    now: chrono::DateTime<Utc>,
    media_job_id: &str,
) -> anyhow::Result<(String, String)> {
    use crate::models::{Discussion, SummaryStrategy};

    let id = uuid::Uuid::new_v4().to_string();
    // Titles are read in a sidebar: a full prompt would be unusable there.
    let title: String = {
        let trimmed = prompt.trim();
        let mut short: String = trimmed.chars().take(60).collect();
        if trimmed.chars().count() > 60 {
            short.push('…');
        }
        short
    };
    let (message_id, message) = build_prompt_message(prompt, now, None, media_job_id);
    let discussion = Discussion {
        awaiting_agent: false,
        agent_running: false,
        id: id.clone(),
        project_id: None,
        title,
        agent: crate::models::AgentType::ClaudeCode,
        language: "fr".into(),
        participants: vec![],
        messages: vec![message.clone()],
        message_count: 1,
        non_system_message_count: 1,
        skill_ids: vec![],
        profile_ids: vec![],
        directive_ids: vec![],
        tier: crate::models::ModelTier::default(),
        model: None,
        pin_first_message: false,
        archived: false,
        pinned: false,
        workspace_mode: "Direct".into(),
        workspace_path: None,
        worktree_branch: None,
        summary_cache: None,
        summary_up_to_msg_idx: None,
        summary_strategy: SummaryStrategy::Off,
        introspection_call_count: 0,
        shared_id: None,
        shared_with: vec![],
        workflow_run_id: None,
        test_mode_restore_branch: None,
        test_mode_stash_ref: None,
        created_at: now,
        updated_at: now,
    };

    crate::db::discussions::insert_discussion(conn, &discussion)?;
    crate::db::discussions::insert_message(conn, &discussion.id, &message)?;
    // The prompt message is the anchor the finished asset attaches to.
    Ok((id, message_id))
}

pub async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<MediaJobView>> {
    let lookup = id.clone();
    match state
        .db
        .with_read_conn(move |conn| media_jobs::get(conn, &lookup))
        .await
    {
        Ok(Some(job)) => Json(ApiResponse::ok(MediaJobView {
            id: job.id,
            modality: job.modality,
            status: job.status,
            discussion_id: job.discussion_id,
            message_id: job.message_id,
            model: job.model,
            context_file_id: job.context_file_id,
            width: job.rendered.width,
            height: job.rendered.height,
            duration_ms: job.rendered.duration_ms,
            cost_usd: job.cost.map(|c| c.cost_usd),
            is_byok: job.cost.map(|c| c.is_byok),
            last_error: job.last_error,
            attempts: job.attempts,
        })),
        Ok(None) => Json(ApiResponse::err(format!("unknown media job: {id}"))),
        Err(e) => Json(ApiResponse::err(format!("failed to read job: {e}"))),
    }
}

pub async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<bool>> {
    let now = Utc::now();
    let job_id = id.clone();
    match state
        .db
        .with_conn(move |conn| media_jobs::cancel(conn, &id, now))
        .await
    {
        // False means it had already settled — a completed generation is
        // billed and must not be rewritten as cancelled.
        Ok(cancelled) => {
            if cancelled {
                // The endpoint itself publishes: relying on a caller to sync
                // afterwards is what made the previous test a false positive.
                if let Err(e) = crate::api::shared_runs::publish_media_job(&state, &job_id).await {
                    tracing::warn!(job = %job_id, error = %e, "media run publication failed");
                }
            }
            Json(ApiResponse::ok(cancelled))
        }
        Err(e) => Json(ApiResponse::err(format!("failed to cancel: {e}"))),
    }
}

#[derive(Debug, Serialize)]
pub struct MediaSpendEntry {
    pub id: String,
    pub modality: MediaModality,
    pub model: String,
    pub discussion_id: Option<String>,
    pub cost_usd: f64,
    pub is_byok: bool,
    pub completed_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MediaSpendResponse {
    /// Per-generation detail, newest first.
    pub entries: Vec<MediaSpendEntry>,
    pub image_total_usd: f64,
    pub video_total_usd: f64,
    pub total_usd: f64,
}

/// Media spend, deliberately its OWN counter.
///
/// A generation is billed per image or per second and its provider usage
/// payload carries no token count at all, so folding this into the token
/// counters would either report zero tokens against real spend or invent a
/// token equivalent. Amounts are the values the provider declared, never
/// recomputed from a published rate — measured drift: 0.0708932 USD billed
/// against 0.0678 implied.
pub async fn spend(State(state): State<AppState>) -> Json<ApiResponse<MediaSpendResponse>> {
    const LIMIT: u32 = 200;
    let entries = match state
        .db
        .with_read_conn(|conn| media_jobs::spend(conn, LIMIT))
        .await
    {
        Ok(rows) => rows,
        Err(e) => return Json(ApiResponse::err(format!("failed to read media spend: {e}"))),
    };
    let (image_total_usd, video_total_usd) =
        match state.db.with_read_conn(media_jobs::spend_total).await {
            Ok(totals) => totals,
            Err(e) => {
                return Json(ApiResponse::err(format!(
                    "failed to total media spend: {e}"
                )))
            }
        };

    Json(ApiResponse::ok(MediaSpendResponse {
        entries: entries
            .into_iter()
            .map(|row| MediaSpendEntry {
                id: row.id,
                modality: row.modality,
                model: row.model,
                discussion_id: row.discussion_id,
                cost_usd: row.cost_usd,
                is_byok: row.is_byok,
                completed_at: row.completed_at,
            })
            .collect(),
        image_total_usd,
        video_total_usd,
        total_usd: image_total_usd + video_total_usd,
    }))
}

#[derive(Debug, Deserialize)]
pub struct EstimateQuery {
    pub connection_id: String,
    pub modality: MediaModality,
    #[serde(default)]
    pub duration_secs: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct MediaEstimate {
    pub model: String,
    /// Absent when nothing comparable was ever billed: no estimate is better
    /// than a fabricated one, and the UI must say "unknown" rather than "free".
    pub estimated_usd: Option<f64>,
    /// How many past generations the estimate is based on. 0 means none.
    pub samples: u32,
}

/// Cost estimate shown BEFORE sending, so a human sees the price of a click.
///
/// Measured from past generations rather than a published rate: the rate does
/// not reproduce the invoice, and a hardcoded table drifts the day pricing
/// changes.
pub async fn estimate(
    State(state): State<AppState>,
    Query(query): Query<EstimateQuery>,
) -> Json<ApiResponse<MediaEstimate>> {
    let lookup = query.connection_id.clone();
    let connection = match state
        .db
        .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &lookup))
        .await
    {
        Ok(Some(connection)) => connection,
        Ok(None) => {
            return Json(ApiResponse::err(format!(
                "unknown connection: {}",
                query.connection_id
            )))
        }
        Err(e) => return Json(ApiResponse::err(format!("failed to read connection: {e}"))),
    };

    let model = match query.modality {
        MediaModality::Image => connection.image_model.clone(),
        MediaModality::Video => connection.video_model.clone(),
    };
    let Some(model) = model.filter(|m| !m.trim().is_empty()) else {
        return Json(ApiResponse::err(format!(
            "connection '{}' has no {} model configured",
            connection.display_name,
            query.modality.as_str()
        )));
    };

    let probe_model = model.clone();
    let modality = query.modality;
    let observed = match state
        .db
        .with_read_conn(move |conn| media_jobs::observed_unit_cost(conn, &probe_model, modality))
        .await
    {
        Ok(observed) => observed,
        Err(e) => return Json(ApiResponse::err(format!("failed to read past cost: {e}"))),
    };

    let (estimated_usd, samples) = match observed {
        Some((unit, samples)) => {
            let estimate = match query.modality {
                // Per-second unit: a duration is required to scale it, and
                // guessing one would misprice the request.
                MediaModality::Video => query.duration_secs.map(|secs| unit * f64::from(secs)),
                MediaModality::Image => Some(unit),
            };
            (estimate, samples)
        }
        None => (None, 0),
    };

    Json(ApiResponse::ok(MediaEstimate {
        model,
        estimated_usd,
        samples,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn launch(job_id: &str, prompt: &str, now: chrono::DateTime<Utc>) -> MediaLaunchWrite {
        MediaLaunchWrite {
            job_id: job_id.to_string(),
            modality: MediaModality::Image,
            connection_id: "conn-1".to_string(),
            model: "stub/image".to_string(),
            prompt: prompt.to_string(),
            params: MediaParams::default(),
            source_message_id: None,
            scheduled_at: now,
            deadline_at: now + chrono::Duration::minutes(20),
        }
    }

    /// KT-549 blocker #1: a job insert failure — here forced by colliding on
    /// the primary key, standing in for any DB failure mid-write — must roll
    /// back the anchor message it was about to be paired with. Without the
    /// transaction, the message from the failing call would survive with no
    /// job ever pointing at it.
    #[tokio::test]
    async fn a_failed_job_insert_leaves_no_orphan_anchor_message() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('disc-1', 'Media', '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let now = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // First launch succeeds: one message, one job.
        db.with_conn(move |conn| {
            write_media_launch(
                conn,
                Some("disc-1".to_string()),
                launch("job-1", "un chat", now),
            )
        })
        .await
        .expect("first launch must succeed");

        // Second launch reuses the SAME job id: `media_jobs.id` is a primary
        // key, so its insert fails and must roll back the anchor message this
        // call also tried to create.
        let failed = db
            .with_conn(move |conn| {
                write_media_launch(
                    conn,
                    Some("disc-1".to_string()),
                    launch("job-1", "un chien", now),
                )
            })
            .await;
        assert!(failed.is_err(), "a colliding job id must fail to insert");

        let message_count: i64 = db
            .with_read_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(
            message_count, 1,
            "the failed launch's message must not survive its job's failure"
        );

        let contents: Vec<String> = db
            .with_read_conn(|conn| {
                let mut stmt = conn.prepare("SELECT content FROM messages")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .await
            .unwrap();
        // The surviving message is the first launch's, not a message from the
        // failed second one.
        assert_eq!(contents, vec!["un chat".to_string()]);

        let job_count: i64 = db
            .with_read_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM media_jobs", [], |row| row.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(
            job_count, 1,
            "the failed insert must not leave a second row"
        );
    }

    #[tokio::test]
    async fn a_source_message_is_preserved_and_each_launch_gets_its_own_anchor() {
        let db = Database::open_in_memory().expect("in-memory db");
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let source_id = db
            .with_conn(move |conn| {
                for id in ["disc-source", "disc-other"] {
                    conn.execute(
                        "INSERT INTO discussions (id, title, created_at, updated_at) \
                         VALUES (?1, 'Media', '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
                        rusqlite::params![id],
                    )?;
                }
                let (source_id, source) = build_prompt_message("source intacte", now, None);
                crate::db::discussions::insert_message(conn, "disc-source", &source)?;
                Ok(source_id)
            })
            .await
            .unwrap();

        let first_source = source_id.clone();
        let first_anchor = db
            .with_conn(move |conn| {
                let mut input = launch("job-source-1", "anime cette image", now);
                input.source_message_id = Some(first_source);
                write_media_launch(conn, Some("disc-source".to_string()), input)
            })
            .await
            .expect("first source launch")
            .1;

        let second_source = source_id.clone();
        let second_anchor = db
            .with_conn(move |conn| {
                let mut input = launch("job-source-2", "une autre animation", now);
                input.source_message_id = Some(second_source);
                write_media_launch(conn, Some("disc-source".to_string()), input)
            })
            .await
            .expect("second source launch")
            .1;

        assert_ne!(
            first_anchor, second_anchor,
            "each job owns a distinct anchor"
        );
        assert_ne!(
            first_anchor, source_id,
            "the source is never reused as an anchor"
        );

        let expected_source = source_id.clone();
        let expected_reply = source_id.clone();
        let expected_first = first_anchor.clone();
        let expected_second = second_anchor.clone();
        db.with_read_conn(move |conn| {
            let source_content: String = conn.query_row(
                "SELECT content FROM messages WHERE id = ?1",
                rusqlite::params![expected_source],
                |row| row.get(0),
            )?;
            assert_eq!(source_content, "source intacte");

            let anchors: Vec<(String, Option<String>, i64)> = {
                let mut stmt = conn.prepare(
                    "SELECT id, reply_to_message_id, sort_order FROM messages \
                     WHERE id IN (?1, ?2) ORDER BY sort_order",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![expected_first, expected_second], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            assert_eq!(anchors.len(), 2);
            assert_eq!(anchors[0].1.as_deref(), Some(expected_reply.as_str()));
            assert_eq!(anchors[1].1.as_deref(), Some(expected_reply.as_str()));
            assert!(
                anchors[0].2 < anchors[1].2,
                "launch order must be monotonic"
            );
            Ok(())
        })
        .await
        .unwrap();

        let cross_source = source_id.clone();
        let cross = db
            .with_conn(move |conn| {
                let mut input = launch("job-cross", "ne doit pas partir", now);
                input.source_message_id = Some(cross_source);
                write_media_launch(conn, Some("disc-other".to_string()), input)
            })
            .await;
        assert!(cross.is_err(), "a cross-discussion source must fail closed");

        let without_disc_source = source_id;
        let without_disc = db
            .with_conn(move |conn| {
                let mut input = launch("job-no-disc", "ne doit pas partir", now);
                input.source_message_id = Some(without_disc_source);
                write_media_launch(conn, None, input)
            })
            .await;
        assert!(without_disc.is_err(), "a source requires its discussion");

        let rejected_rows: i64 = db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM media_jobs WHERE id IN ('job-cross', 'job-no-disc')",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            rejected_rows, 0,
            "validation errors must not leave jobs behind"
        );
    }
}
