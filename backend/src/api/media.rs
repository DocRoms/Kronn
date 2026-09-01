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
    /// Message the asset belongs to. Optional: when absent the newest message
    /// of the discussion is used, so the asset lands on the turn that asked
    /// for it instead of staying pending.
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
}

#[derive(Debug, Serialize)]
pub struct MediaJobView {
    pub id: String,
    pub modality: MediaModality,
    pub status: MediaJobStatus,
    pub model: String,
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

    // Resolved BEFORE inserting: the job and its shared run must carry the real
    // scope, otherwise media runs vanish from project views. A named discussion
    // that does not exist is refused rather than producing an orphan asset
    // nobody can reach; no discussion at all gets one created.
    // Message the finished asset will hang from.
    let mut anchor: Option<String> = None;
    let (discussion_id, project_id) = match req.discussion_id.clone() {
        Some(named) => {
            let lookup = named.clone();
            match state
                .db
                .with_read_conn(move |conn| {
                    conn.query_row(
                        "SELECT project_id FROM discussions WHERE id = ?1",
                        rusqlite::params![lookup],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()
                    .map_err(Into::into)
                })
                .await
            {
                Ok(Some(project_id)) => (named, project_id),
                Ok(None) => return Json(ApiResponse::err(format!("unknown discussion: {named}"))),
                Err(e) => return Json(ApiResponse::err(format!("failed to read discussion: {e}"))),
            }
        }
        None => match create_media_discussion(&state, &prompt).await {
            Ok((id, launch_message_id)) => {
                anchor = Some(launch_message_id);
                (id, None)
            }
            Err(e) => {
                return Json(ApiResponse::err(format!(
                    "failed to create a discussion for this generation: {e}"
                )))
            }
        },
    };

    // Resolved BEFORE the job is queued: reading it at completion would let a
    // message written during the 100 s generation capture the asset.
    if anchor.is_none() {
        anchor = req.message_id.clone();
    }
    if anchor.is_none() {
        let did = discussion_id.clone();
        anchor = state
            .db
            .with_read_conn(move |conn| Ok(crate::db::discussions::latest_message_id(conn, &did)?))
            .await
            .unwrap_or(None);
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

    let now = Utc::now();
    let deadline =
        now + chrono::Duration::from_std(DEFAULT_DEADLINE).unwrap_or(chrono::Duration::minutes(20));
    let job_id = uuid::Uuid::new_v4().to_string();
    let params = MediaParams {
        duration_secs: req.duration_secs,
        resolution: req.resolution.clone(),
        aspect_ratio: req.aspect_ratio.clone(),
        generate_audio: req.generate_audio,
    };

    let insert = {
        let job_id = job_id.clone();
        let connection_id = connection.id.clone();
        let model = model.clone();
        let discussion_id = discussion_id.clone();
        let project_id = project_id.clone();
        move |conn: &rusqlite::Connection| {
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: &job_id,
                    modality: req.modality,
                    connection_id: &connection_id,
                    model: &model,
                    prompt: &prompt,
                    params: &params,
                    discussion_id: Some(&discussion_id),
                    message_id: anchor.as_deref(),
                    project_id: project_id.as_deref(),
                    scheduled_at: now,
                    deadline_at: deadline,
                },
                now,
            )?;
            Ok(())
        }
    };
    if let Err(e) = state.db.with_conn(insert).await {
        return Json(ApiResponse::err(format!("failed to queue job: {e}")));
    }
    // Published through the single point, so the run is visible AND broadcast
    // while the job is still queued — a 100 s generation must not be invisible
    // until the provider answers.
    if let Err(e) = crate::api::shared_runs::publish_media_job(&state, &job_id).await {
        tracing::warn!(job = %job_id, error = %e, "media run publication failed");
    }

    Json(ApiResponse::ok(GenerateMediaResponse {
        job_id,
        status: MediaJobStatus::Pending,
        model,
    }))
}

/// Creates the discussion an asset needs when the caller has none.
///
/// The prompt becomes the first message, so the room explains itself: a
/// discussion holding a generated video with no trace of what was asked would
/// be unreadable a day later.
async fn create_media_discussion(
    state: &AppState,
    prompt: &str,
) -> anyhow::Result<(String, String)> {
    use crate::models::{
        Discussion, DiscussionMessage, MessageChannel, MessageRole, SummaryStrategy,
    };

    let now = Utc::now();
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
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    };
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

    let disc = discussion.clone();
    state
        .db
        .with_conn(move |conn| {
            // One transaction: a discussion without its prompt would be worse
            // than no discussion at all.
            crate::db::discussions::insert_discussion(conn, &disc)?;
            crate::db::discussions::insert_message(conn, &disc.id, &message)?;
            Ok(())
        })
        .await?;
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
