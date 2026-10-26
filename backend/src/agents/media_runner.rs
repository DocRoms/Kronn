//! I/O side of a media job: submit, poll, download, persist.
//!
//! Ordering is decided by [`media_worker::next_action`] and kept out of here;
//! this module only performs what was decided. Two invariants it must never
//! break, because both cost money:
//!   * the provider handle is recorded BEFORE the job is rescheduled, so a
//!     crash in between resumes polling instead of paying for a second
//!     generation;
//!   * the asset is downloaded and persisted server-side, so no signed URL is
//!     ever stored or handed to a browser.

use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use base64::Engine;
use chrono::{DateTime, Utc};

use crate::agents::media_asset_url::{validate_asset_url, ValidatedAssetUrl};
use crate::agents::media_codec::{MediaCodec, MediaPollState};
use crate::agents::media_worker::{backoff_delay, next_action, MediaAction};
use crate::db::media_jobs::{self, Completion, MediaJob};
use crate::db::Database;
use crate::models::{MediaCost, MediaJobStatus, MediaModality, MediaRendered};

/// Everything one advance needs, so the caller owns credential lookup and the
/// job never carries a secret.
pub struct MediaContext<'a> {
    pub codec: &'a dyn MediaCodec,
    /// Connection base URL as configured.
    pub base: &'a str,
    pub api_key: &'a str,
    pub client: &'a reqwest::Client,
}

/// Caps a downloaded asset so a runaway provider cannot fill the disk.
const MAX_ASSET_BYTES: usize = 256 * 1024 * 1024;

/// Advances one claimed job by exactly one step and returns its new status.
pub async fn advance(
    db: &Database,
    ctx: MediaContext<'_>,
    job: &MediaJob,
    now: DateTime<Utc>,
) -> Result<MediaJobStatus> {
    if !ctx.codec.supports(job.modality) {
        // Refused here rather than dispatched to a chat endpoint, which is
        // what currently answers a bare HTTP 500.
        let message = format!("provider does not support {}", job.modality.as_str());
        settle_failure(db, &job.id, MediaJobStatus::Failed, &message, now).await?;
        return Ok(MediaJobStatus::Failed);
    }

    match next_action(job, now) {
        // Nothing is billed twice on a local server: ask it again.
        MediaAction::RefuseUnsafeResume if ctx.codec.resubmit_is_free() => {
            submit(db, ctx, job, now).await
        }
        MediaAction::RefuseUnsafeResume => {
            // Deliberately terminal: the provider may have accepted — and
            // charged — a submission whose handle we lost. Retrying blind is
            // the one outcome worse than failing.
            settle_failure(
                db,
                &job.id,
                MediaJobStatus::Failed,
                "a generation was submitted but its provider handle was lost \
                 (likely a restart mid-request). Not retried automatically to \
                 avoid paying twice — relaunch it explicitly if nothing arrived.",
                now,
            )
            .await?;
            Ok(MediaJobStatus::Failed)
        }
        MediaAction::Expire => {
            settle_failure(
                db,
                &job.id,
                MediaJobStatus::TimedOut,
                "deadline exceeded",
                now,
            )
            .await?;
            Ok(MediaJobStatus::TimedOut)
        }
        MediaAction::Submit => submit(db, ctx, job, now).await,
        MediaAction::Poll => poll(db, ctx, job, now).await,
    }
}

async fn submit(
    db: &Database,
    ctx: MediaContext<'_>,
    job: &MediaJob,
    now: DateTime<Utc>,
) -> Result<MediaJobStatus> {
    match job.modality {
        // Images come back in the same response, so there is no handle to
        // record and nothing to resume.
        MediaModality::Image if !ctx.codec.image_is_async() => {
            let url = ctx.codec.image_url(ctx.base);
            // Same rule as video: read at execution time from the ids the job
            // stored, so a restart resumes with the same pictures.
            let references = match load_references(db, job).await {
                Ok(references) => references,
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            let body = match ctx
                .codec
                .image_body(&job.model, &job.prompt, &job.params, &references)
            {
                Ok(body) => body,
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            // Stamped BEFORE the billable request leaves, and committed, so a
            // crash in flight cannot look like "never submitted".
            mark_attempt(db, &job.id, now).await?;
            let text = send_json(ctx.client, &url, ctx.api_key, &body).await?;
            let response = match ctx.codec.parse_image_response(&text) {
                Ok(response) => response,
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            let first = response
                .images
                .first()
                .ok_or_else(|| anyhow!("image response carried no payload"))?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(first)?;
            // Image generation is billed like video, and the figure only
            // exists in this response: persisted here or lost.
            persist(
                db,
                ctx,
                job,
                bytes,
                response.cost,
                response.generation_id.as_deref(),
                now,
            )
            .await?;
            Ok(MediaJobStatus::Completed)
        }
        // Videos, and images on a provider that queues them (ComfyUI).
        modality => {
            let url = match modality {
                MediaModality::Image => ctx.codec.image_url(ctx.base),
                MediaModality::Video => ctx.codec.video_submit_url(ctx.base),
            };
            // Read at execution time from the id the job stored, so a restart
            // resumes with the same source picture instead of a copy that
            // could have been taken before the file was even final.
            let references = match load_references(db, job).await {
                Ok(references) => references,
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            let built = match modality {
                MediaModality::Image => {
                    ctx.codec
                        .image_body(&job.model, &job.prompt, &job.params, &references)
                }
                // Every video model advertises a single frame; sending more
                // would silently drop all but one.
                MediaModality::Video => ctx.codec.video_body_with_references(
                    &job.model,
                    &job.prompt,
                    &job.params,
                    &references,
                ),
            };
            let body = match built {
                Ok(body) => body,
                // A provider that cannot take this source image must fail
                // BEFORE the submission: a text-to-video billed in place of
                // the requested image-to-video is a charge for something
                // nobody asked for.
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            mark_attempt(db, &job.id, now).await?;
            let text = send_json(ctx.client, &url, ctx.api_key, &body).await?;
            let ack = match ctx.codec.parse_submit_response(&text) {
                Ok(ack) => ack,
                Err(e) => {
                    settle_failure(db, &job.id, MediaJobStatus::Failed, &e.to_string(), now)
                        .await?;
                    return Ok(MediaJobStatus::Failed);
                }
            };
            // Recorded FIRST: from this point the provider is generating and
            // charging, so a crash must resume polling, not resubmit.
            let id = job.id.clone();
            let handle = ack.provider_job_id.clone();
            db.with_conn(move |conn| {
                media_jobs::record_submission(conn, &id, &handle, now)?;
                Ok(())
            })
            .await?;
            reschedule(db, &job.id, ctx.codec.poll_delay(job.attempts), now).await?;
            Ok(MediaJobStatus::Pending)
        }
    }
}

async fn poll(
    db: &Database,
    ctx: MediaContext<'_>,
    job: &MediaJob,
    now: DateTime<Utc>,
) -> Result<MediaJobStatus> {
    let handle = job
        .provider_job_id
        .clone()
        .ok_or_else(|| anyhow!("polling a job without a provider handle"))?;
    let url = ctx.codec.video_poll_url(ctx.base, &handle);
    let text = get_text(ctx.client, &url, ctx.api_key).await?;

    match ctx.codec.parse_poll_response(&text) {
        Ok(MediaPollState::Pending) => {
            reschedule(db, &job.id, ctx.codec.poll_delay(job.attempts), now).await?;
            Ok(MediaJobStatus::Pending)
        }
        Ok(MediaPollState::Completed {
            urls,
            cost,
            generation_id,
        }) => {
            let download = urls
                .first()
                .cloned()
                .ok_or_else(|| anyhow!("completed job carried no download url"))?;
            // The URL comes from the provider payload, so it is untrusted
            // input: cleared against the codec policy before it is fetched,
            // and the credential travels only where that policy vouches for.
            let cleared = validate_asset_url(&download, ctx.base, &ctx.codec.asset_host_policy())?;
            let bytes = download_asset(&cleared, ctx.api_key, DOWNLOAD_PUBLIC).await?;
            persist(db, ctx, job, bytes, cost, generation_id.as_deref(), now).await?;
            Ok(MediaJobStatus::Completed)
        }
        Ok(MediaPollState::Failed { message }) => {
            settle_failure(db, &job.id, MediaJobStatus::Failed, &message, now).await?;
            Ok(MediaJobStatus::Failed)
        }
        Err(e) => {
            // An unreadable poll is not a failed generation: keep polling
            // until the deadline rather than discarding a paid job.
            reschedule(db, &job.id, ctx.codec.poll_delay(job.attempts), now).await?;
            tracing::warn!(job = %job.id, error = %e, "media poll response unreadable");
            Ok(MediaJobStatus::Pending)
        }
    }
}

/// Stores the asset as a context file and settles the job.
async fn persist(
    db: &Database,
    ctx: MediaContext<'_>,
    job: &MediaJob,
    bytes: Vec<u8>,
    cost: Option<MediaCost>,
    generation_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    let discussion_id = job
        .discussion_id
        .clone()
        .ok_or_else(|| anyhow!("media job has no discussion to attach its asset to"))?;
    let file_id = uuid::Uuid::new_v4().to_string();
    let (extension, mime) = match job.modality {
        MediaModality::Image => ("png", "image/png"),
        MediaModality::Video => ("mp4", "video/mp4"),
    };
    let filename = format!("{}-{}.{}", job.modality.as_str(), &file_id[..8], extension);
    // Dimensions come from the produced file: a "480p 16:9" request came back
    // as 864x496, so the requested parameters describe nothing.
    let rendered = probe_rendered(&bytes, job.modality);
    let size = bytes.len() as u64;
    // Stored like an upload: durable copy served by the UI, mirror the agent can open.
    let dirs = crate::core::context_files::discussion_attachment_dirs(db, &discussion_id).await;
    let disk_path = crate::core::context_files::save_file_to_dir_or_disk(
        &dirs.durable,
        &file_id,
        &filename,
        &bytes,
    )?;
    if let Some(agent_dir) = dirs.agent.as_deref() {
        crate::core::context_files::agent_readable_path(&disk_path, agent_dir);
    }

    let job_id = job.id.clone();
    let anchor = job.message_id.clone();
    let generation_id = generation_id.map(str::to_string);
    let rendered_for_db = rendered.clone();
    let file_id_for_db = file_id.clone();
    db.with_conn(move |conn| {
        // original_size is what KT-541 sums, so it must be the real byte count.
        crate::db::discussions::insert_context_file(
            conn,
            &file_id_for_db,
            &discussion_id,
            &filename,
            mime,
            size,
            "",
            Some(&disk_path),
        )?;
        // Anchored in the same transaction as the insert. A generated asset
        // left with `message_id IS NULL` reads as "pending upload" everywhere,
        // and the next human message would silently claim it.
        if let Some(message_id) = anchor.as_deref() {
            crate::db::discussions::anchor_context_file_to_message(
                conn,
                &file_id_for_db,
                message_id,
            )?;
        }
        media_jobs::complete(
            conn,
            &job_id,
            Completion {
                context_file_id: &file_id_for_db,
                rendered: &rendered_for_db,
                cost,
                generation_id: generation_id.as_deref(),
            },
            now,
        )?;
        Ok(())
    })
    .await?;
    let _ = ctx;
    Ok(())
}

async fn mark_attempt(db: &Database, job_id: &str, now: DateTime<Utc>) -> Result<()> {
    let job_id = job_id.to_string();
    db.with_conn(move |conn| {
        media_jobs::mark_submit_attempt(conn, &job_id, now)?;
        Ok(())
    })
    .await
}

async fn settle_failure(
    db: &Database,
    job_id: &str,
    status: MediaJobStatus,
    message: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    let job_id = job_id.to_string();
    let message = message.to_string();
    db.with_conn(move |conn| {
        media_jobs::fail(conn, &job_id, status, &message, now)?;
        Ok(())
    })
    .await
}

async fn reschedule(
    db: &Database,
    job_id: &str,
    delay: Duration,
    now: DateTime<Utc>,
) -> Result<()> {
    let next = now + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::seconds(30));
    let job_id = job_id.to_string();
    db.with_conn(move |conn| {
        media_jobs::reschedule(conn, &job_id, next, now)?;
        Ok(())
    })
    .await
}

async fn send_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<String> {
    let response = client
        .post(url)
        .bearer_auth(api_key)
        .json(body)
        .timeout(Duration::from_secs(120))
        .send()
        .await?;
    read_body(response, url).await
}

async fn get_text(client: &reqwest::Client, url: &str, api_key: &str) -> Result<String> {
    let response = client
        .get(url)
        .bearer_auth(api_key)
        .timeout(Duration::from_secs(60))
        .send()
        .await?;
    read_body(response, url).await
}

/// The provider's own error text is kept, but the URL is not echoed back: it
/// can carry a job handle we do not want in a stored diagnostic.
async fn read_body(response: reqwest::Response, url: &str) -> Result<String> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() && text.trim().is_empty() {
        bail!("provider answered HTTP {status} with an empty body");
    }
    tracing::debug!(status = %status, endpoint = %redact(url), "media provider response");
    Ok(text)
}

/// Policy for an asset off the configured origin.
const DOWNLOAD_PUBLIC: crate::core::safe_http::SafeHttpPolicy =
    crate::core::safe_http::SafeHttpPolicy::Public;

/// Download of the finished asset. Server-side on purpose: the provider URL
/// still requires the credential, and must never reach a browser.
///
/// The URL comes from the provider's payload. On the configured origin (which
/// may be a local gateway) no redirect may leave that origin; anywhere else
/// every hop is held to `public` and a cross-origin hop loses the credential.
async fn download_asset(
    cleared: &ValidatedAssetUrl,
    api_key: &str,
    public: crate::core::safe_http::SafeHttpPolicy,
) -> Result<Vec<u8>> {
    use crate::core::safe_http::{self, ClientOptions, Outbound, Redirects, SafeHttpPolicy};
    let url = reqwest::Url::parse(&cleared.url)?;
    let (policy, redirects) = if cleared.on_configured_origin {
        (SafeHttpPolicy::Configured, Redirects::SameOrigin)
    } else {
        (public, Redirects::Manual)
    };
    let client = safe_http::client(
        policy,
        ClientOptions::new(redirects).timeout(Duration::from_secs(300)),
    )
    .map_err(|e| anyhow!(e))?;
    let mut headers = reqwest::header::HeaderMap::new();
    // A pre-signed URL is its own authorisation: attaching the key there would
    // hand it to whoever serves that host.
    if cleared.send_credential {
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {api_key}"))?;
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    let response = if cleared.on_configured_origin {
        client.get(url).headers(headers).send().await
    } else {
        let attach = |request: safe_http::SafeRequest| request;
        safe_http::send_following(
            &client,
            Outbound {
                method: reqwest::Method::GET,
                url,
                headers,
                secret_headers: &[reqwest::header::AUTHORIZATION],
                secret_query_keys: &[],
                attach_body: &attach,
                has_body: false,
                pinned_base: None,
                hop_guard: None,
            },
        )
        .await
    }
    .map_err(|e| anyhow!("asset download failed: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("asset download answered HTTP {status}");
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_ASSET_BYTES {
        bail!(
            "asset is {} bytes, over the {MAX_ASSET_BYTES} cap",
            bytes.len()
        );
    }
    if bytes.is_empty() {
        bail!("asset download returned no bytes");
    }
    Ok(bytes.to_vec())
}

/// Drops everything after the host so a diagnostic never carries a job handle.
fn redact(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split('/').next().unwrap_or("");
            format!("{scheme}://{host}/…")
        }
        None => "…".to_string(),
    }
}

/// Reads the real dimensions out of the produced file. Best-effort: an
/// unreadable header leaves them absent rather than guessed.
fn probe_rendered(bytes: &[u8], modality: MediaModality) -> MediaRendered {
    match modality {
        MediaModality::Video => crate::core::media_probe::probe_mp4(bytes),
        MediaModality::Image => crate::core::media_probe::probe_image(bytes),
    }
}

/// Why an advance failed, which decides what happens to the job.
///
/// The distinction is not cosmetic: `due()` only picks up `pending`, so a job
/// left `running` after an error is frozen until a restart or its deadline —
/// on a generation the provider is already billing. Every failure must
/// therefore settle the job one way or the other.
#[derive(Debug)]
enum AdvanceFailure {
    /// Worth another attempt: network, timeout, unreadable provider answer.
    /// The job goes back to `pending` with backoff and resumes on its own.
    Transient(String),
    /// Retrying cannot help: missing connection, missing credential, no
    /// endpoint, unsupported modality. The job fails with an actionable
    /// diagnostic instead of silently burning its deadline.
    Permanent(String),
}

impl AdvanceFailure {
    fn message(&self) -> &str {
        match self {
            Self::Transient(message) | Self::Permanent(message) => message,
        }
    }
}

/// How often the loop looks for due jobs. Individual jobs carry their own
/// backoff, so this only bounds how late a ready job is picked up.
const TICK: Duration = Duration::from_secs(5);
/// Jobs advanced per tick, so one busy discussion cannot starve the others.
const BATCH: u32 = 4;

/// Background loop: reclaims what a previous process left behind, then keeps
/// advancing due jobs.
///
/// Reclaiming FIRST is the point of the whole design: a job left `running` by a
/// killed process is already generating and already billed, so it must resume
/// polling rather than be abandoned or paid for twice.
pub async fn run_loop(state: crate::AppState) {
    let now = Utc::now();
    match state
        .db
        .with_conn(move |conn| crate::db::media_jobs::reclaim_orphans(conn, now))
        .await
    {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "media jobs reclaimed after restart"),
        Err(e) => tracing::warn!(error = %e, "media job reclaim failed"),
    }

    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::error!(error = %e, "media worker could not build its HTTP client");
            return;
        }
    };

    loop {
        tokio::time::sleep(TICK).await;
        if let Err(e) = tick(&state, &client).await {
            // A failing tick must not kill the loop: the next one retries, and
            // each job keeps its own deadline.
            tracing::warn!(error = %e, "media worker tick failed");
        }
    }
}

/// One sweep. Public so a test can exercise the REAL path instead of
/// recreating the projection by hand, which proves the mapper and not the loop.
pub async fn tick(state: &crate::AppState, client: &reqwest::Client) -> Result<()> {
    let now = Utc::now();
    let expired = state
        .db
        .with_conn(move |conn| crate::db::media_jobs::expire_overdue(conn, now))
        .await?;
    if !expired.is_empty() {
        tracing::info!(count = expired.len(), "media jobs timed out");
    }
    // Published explicitly: an expired job leaves `pending`, so `due()` below
    // will never see it again and no later sweep would catch up. Without this,
    // a card already on screen keeps showing `running`.
    for job_id in &expired {
        crate::api::comfyui::release_local_job(state, job_id).await;
        if let Err(e) = crate::api::shared_runs::publish_media_job(state, job_id).await {
            tracing::warn!(job = %job_id, error = %e, "expired media run publication failed");
        }
    }

    let due = state
        .db
        .with_read_conn(move |conn| crate::db::media_jobs::due(conn, now, BATCH))
        .await?;

    // Claimed first and sequentially: `claim` is the single atomic
    // compare-and-swap that stops two workers billing the same generation
    // twice, and it is a fast local UPDATE — nothing here waits on a
    // provider.
    let mut claimed_jobs = Vec::new();
    for job in due {
        let id = job.id.clone();
        let claimed = state
            .db
            .with_conn(move |conn| crate::db::media_jobs::claim(conn, &id, now))
            .await?;
        if claimed {
            claimed_jobs.push(job);
        }
        // Not claimed: another worker took it first; never run a billed
        // generation twice.
    }

    // Advanced CONCURRENTLY: a slow video submission must not block two image
    // jobs claimed in the same sweep from completing (KT-549). Bounded by
    // construction — `due()` never returns more than `BATCH` rows — so no
    // extra semaphore is needed. DB writes inside each advance still
    // serialize on the single connection mutex; only the provider I/O
    // actually overlaps, which is the point.
    let advances = claimed_jobs.into_iter().map(|job| async move {
        let outcome = advance_claimed(state, client, &job, now).await;
        if let Err(failure) = &outcome {
            settle_failed_advance(&state.db, &job, failure, now).await?;
        }
        // Published from the STORED job after every durable transition — the
        // advance may have written a handle, a cost or an error the local copy
        // does not carry. Going through the single point guarantees the
        // broadcast is not forgotten.
        if let Err(e) = crate::api::shared_runs::publish_media_job(state, &job.id).await {
            tracing::warn!(job = %job.id, error = %e, "media run publication failed");
        }
        Ok::<(), anyhow::Error>(())
    });
    for result in futures::future::join_all(advances).await {
        result?;
    }
    Ok(())
}

/// Applies the failure policy so a claimed job is never left frozen in
/// `running`, which `due()` would never pick up again.
async fn settle_failed_advance(
    db: &Database,
    job: &MediaJob,
    failure: &AdvanceFailure,
    now: DateTime<Utc>,
) -> Result<()> {
    let job_id = job.id.clone();
    let message = failure.message().to_string();
    match failure {
        AdvanceFailure::Transient(_) => {
            tracing::warn!(job = %job.id, error = %message, "media advance failed, retrying");
            let delay = backoff_delay(job.attempts);
            let next =
                now + chrono::Duration::from_std(delay).unwrap_or(chrono::Duration::seconds(30));
            db.with_conn(move |conn| {
                crate::db::media_jobs::reschedule(conn, &job_id, next, now)?;
                Ok(())
            })
            .await?;
        }
        AdvanceFailure::Permanent(_) => {
            tracing::warn!(job = %job.id, error = %message, "media advance failed permanently");
            db.with_conn(move |conn| {
                crate::db::media_jobs::fail(
                    conn,
                    &job_id,
                    crate::models::MediaJobStatus::Failed,
                    &message,
                    now,
                )?;
                Ok(())
            })
            .await?;
        }
    }
    Ok(())
}

async fn advance_claimed(
    state: &crate::AppState,
    client: &reqwest::Client,
    job: &MediaJob,
    now: DateTime<Utc>,
) -> std::result::Result<(), AdvanceFailure> {
    if job.connection_id == crate::core::comfyui::CONNECTION_ID {
        return advance_local_comfyui(state, client, job, now).await;
    }
    let connection_id = job.connection_id.clone();
    let connection = state
        .db
        .with_read_conn(move |conn| crate::db::external_api_connections::get(conn, &connection_id))
        .await
        // A failing read is worth retrying; a connection that no longer exists
        // never will be.
        .map_err(|e| AdvanceFailure::Transient(format!("connection lookup failed: {e}")))?
        .ok_or_else(|| {
            AdvanceFailure::Permanent(
                "the connection this job was queued on no longer exists".into(),
            )
        })?;

    let base = connection
        .media_endpoint
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| connection.endpoint.clone())
        .ok_or_else(|| {
            AdvanceFailure::Permanent("this connection has no endpoint configured".into())
        })?;

    let api_key = state
        .config
        .read()
        .await
        .tokens
        .active_key_for(&connection.credential_slug)
        .map(str::to_string)
        .ok_or_else(|| {
            AdvanceFailure::Permanent(
                "this connection has no stored credential — add its API key".into(),
            )
        })?;

    // The codec follows the connection's provider: OpenRouter's proprietary
    // shapes and NVIDIA's visual routes are not interchangeable, and NVIDIA
    // does not even serve media from the host stored on the connection.
    let codec = crate::agents::media_codec::codec_for(connection.origin_preset);
    // A job queued before this connection could be checked — or moved to
    // another provider since — must not spend its deadline retrying a refusal
    // no attempt can lift.
    if !codec.accepts_reference_images() && !job.params.reference_ids().is_empty() {
        return Err(AdvanceFailure::Permanent(
            "this connection's provider does not accept a source image".into(),
        ));
    }
    let ctx = MediaContext {
        codec: codec.as_ref(),
        base: &base,
        api_key: &api_key,
        client,
    };
    // Anything failing here is an I/O or provider-shape problem: retryable
    // until the deadline decides otherwise.
    let status = advance(&state.db, ctx, job, now)
        .await
        .map_err(|e| AdvanceFailure::Transient(e.to_string()))?;
    tracing::debug!(job = %job.id, status = ?status, "media job advanced");

    Ok(())
}

/// A job on the local ComfyUI (KT-1381): no stored connection and no
/// credential; the address comes from the config and a video's text encoder
/// and VAE from what ComfyUI holds when it is submitted.
async fn advance_local_comfyui(
    state: &crate::AppState,
    client: &reqwest::Client,
    job: &MediaJob,
    now: DateTime<Utc>,
) -> std::result::Result<(), AdvanceFailure> {
    let mut config = state.config.read().await.comfyui.clone();
    if let Some(context) = &job.params.comfyui_reference_context {
        config.endpoint = Some(context.endpoint.clone());
        let folder = crate::core::comfyui::folder_of(context.family);
        config
            .model_families
            .insert(format!("{folder}/{}", job.model), context.family);
        match job.modality {
            MediaModality::Image => config.image_settings = context.settings.clone(),
            MediaModality::Video => config.video_settings = context.settings.clone(),
        }
    }
    let base = crate::api::comfyui::base_url(&config);
    let submitting = job.provider_job_id.as_deref().is_none_or(str::is_empty);
    // A stopped ComfyUI is told at once rather than retried until the
    // deadline, and a job it lost (restart, cleared queue) is never seen
    // finishing.
    if now < job.deadline_at {
        if submitting {
            if crate::api::comfyui::probe_version(&base).await.is_none() {
                return Err(AdvanceFailure::Permanent(format!(
                    "ComfyUI does not answer at {base}: start it, then retry the generation"
                )));
            }
        } else if let Some(handle) = job.provider_job_id.as_deref() {
            match crate::api::comfyui::prompt_known(&base, handle).await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(AdvanceFailure::Permanent(
                        "ComfyUI no longer knows this generation (it was restarted or \
                         its queue was cleared): retry it"
                            .into(),
                    ))
                }
                Err(e) => {
                    return Err(AdvanceFailure::Permanent(format!(
                        "ComfyUI stopped answering at {base} during the generation: {e}"
                    )))
                }
            }
        }
    }
    // The workflow and its settings, read when the job is submitted: the
    // user's choice for this file and their overrides for this slot.
    let (image_family, settings) = match job.modality {
        MediaModality::Image => {
            // Saved with the slot, so a failing header read cannot make a
            // known model look unknown; read from ComfyUI only for a model
            // that is no longer the slot's.
            let saved =
                crate::api::comfyui::reference_family(&config, &job.model, MediaModality::Image);
            let family = match saved {
                Some(family) => Some(family),
                None if submitting => {
                    crate::api::comfyui::model_family(
                        &base,
                        &config,
                        crate::core::comfyui::IMAGE_FOLDER,
                        &job.model,
                    )
                    .await
                }
                None => None,
            };
            // Known filenames also resolve old slots without reading a header.
            if submitting {
                if let Some(family) = family {
                    crate::api::comfyui::remember_image_family(state, &job.model, family).await;
                }
            }
            let settings = family
                .unwrap_or(crate::core::comfyui::ComfyFamily::Sdxl)
                .settings(&config.image_settings);
            (family, settings)
        }
        MediaModality::Video => (
            None,
            crate::core::comfyui::ComfyFamily::Wan21T2v.settings(&config.video_settings),
        ),
    };
    let video_companions = if submitting && job.modality == MediaModality::Video {
        match crate::api::comfyui::video_companions(&base).await {
            Ok(companions) => Some(companions),
            // ComfyUI answered and a file is missing: no retry will add it.
            Err(e) if e.downcast_ref::<reqwest::Error>().is_none() => {
                return Err(AdvanceFailure::Permanent(e.to_string()))
            }
            Err(e) => {
                return Err(AdvanceFailure::Transient(format!(
                    "ComfyUI is not reachable at {base}: {e}"
                )))
            }
        }
    } else {
        None
    };
    let video_family = config
        .chosen_family(crate::core::comfyui::VIDEO_FOLDER, &job.model)
        .or_else(|| {
            crate::core::comfyui::family_of(
                crate::core::comfyui::VIDEO_FOLDER,
                &job.model,
                None,
                None,
            )
        });
    let source_images = if submitting
        && now < job.deadline_at
        && !job.params.reference_ids().is_empty()
    {
        let capabilities =
            crate::api::comfyui::reference_capabilities(&config, &job.model, job.modality).await;
        if let Some(reason) = capabilities
            .reference_support
            .as_ref()
            .and_then(|support| support.blocker.as_ref())
        {
            return Err(AdvanceFailure::Permanent(reason.clone()));
        }
        let references = load_references(&state.db, job)
            .await
            .map_err(|error| AdvanceFailure::Permanent(error.to_string()))?;
        upload_comfyui_references(client, &base, &job.id, &references)
            .await
            .map_err(|error| {
                AdvanceFailure::Permanent(format!("ComfyUI source-image upload failed: {error}"))
            })?
    } else {
        Vec::new()
    };
    let codec = crate::agents::media_codec::ComfyUiMediaCodec {
        base: base.clone(),
        seed: crate::core::comfyui::seed_for(&job.id),
        image_family,
        settings,
        video_companions,
        video_family,
        source_images,
        denoise: config.image_settings.denoise.unwrap_or(0.55),
    };
    let ctx = MediaContext {
        codec: &codec,
        base: &base,
        api_key: "",
        client,
    };
    let status = advance(&state.db, ctx, job, now)
        .await
        .map_err(|e| AdvanceFailure::Transient(e.to_string()))?;
    tracing::debug!(job = %job.id, status = ?status, "local ComfyUI media job advanced");
    Ok(())
}

async fn upload_comfyui_references(
    client: &reqwest::Client,
    base: &str,
    job_id: &str,
    references: &[crate::agents::media_codec::MediaReferenceImage],
) -> Result<Vec<String>> {
    use sha2::{Digest, Sha256};
    let job_key: String = Sha256::digest(job_id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let mut names = Vec::with_capacity(references.len());
    for (index, source) in references.iter().enumerate() {
        let filename = format!("kronn-{job_key}-{index}.png");
        let part = reqwest::multipart::Part::bytes(source.bytes.clone())
            .file_name(filename.clone())
            .mime_str(&source.mime_type)?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("type", "input")
            .text("overwrite", "true");
        let body: serde_json::Value = client
            .post(format!("{}/upload/image", base.trim_end_matches('/')))
            .multipart(form)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if body["name"].as_str() != Some(filename.as_str())
            || body["type"].as_str() != Some("input")
            || body["subfolder"]
                .as_str()
                .is_some_and(|folder| !folder.is_empty())
        {
            bail!("ComfyUI returned an unexpected source-image name or location");
        }
        names.push(filename);
    }
    Ok(names)
}

/// Reads the source image a job named, at the moment it is needed.
///
/// The job stores an id, never bytes and never a path: the file is re-read on
/// every attempt, so a retry after a restart uses the same picture, and a file
/// that disappeared fails with a sentence instead of submitting a generation
/// silently stripped of what made it the requested one.
async fn load_references(
    db: &Database,
    job: &MediaJob,
) -> Result<Vec<crate::agents::media_codec::MediaReferenceImage>> {
    let asset_ids = job.params.reference_ids();
    let Some(mode) = job.params.reference_mode else {
        return Ok(Vec::new());
    };
    let mut loaded = Vec::with_capacity(asset_ids.len());
    for asset_id in asset_ids {
        let lookup = asset_id.clone();
        let file = db
            .with_read_conn(move |conn| {
                Ok(crate::db::discussions::get_context_file(conn, &lookup)?)
            })
            .await?
            .ok_or_else(|| anyhow!("a source image of this generation no longer exists"))?;
        // Re-checked here and not only at request time: the file could have
        // been replaced or unlinked between the launch and this attempt, and a
        // job must never read something outside the discussion it belongs to.
        if job
            .discussion_id
            .as_deref()
            .is_some_and(|discussion| discussion != file.discussion_id)
        {
            bail!("a source image no longer belongs to this discussion");
        }
        let path = file
            .disk_path
            .ok_or_else(|| anyhow!("a source image has no stored file"))?;
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| anyhow!("a source image could not be read: {e}"))?;
        loaded.push(crate::agents::media_codec::MediaReferenceImage {
            mode,
            mime_type: file.mime_type,
            bytes,
        });
    }
    Ok(loaded)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uploads_preserve_all_three_sources_order_and_names_on_retry_and_refuse_a_wrong_location(
    ) {
        use sha2::{Digest, Sha256};
        use wiremock::{
            matchers::{body_string_contains, method, path},
            Mock, MockServer, ResponseTemplate,
        };
        let server = MockServer::start().await;
        let sources: Vec<_> = ["red-source", "green-source", "blue-source"]
            .into_iter()
            .map(|text| crate::agents::media_codec::MediaReferenceImage {
                mode: crate::models::MediaReferenceMode::Reference,
                mime_type: "image/png".into(),
                bytes: text.as_bytes().to_vec(),
            })
            .collect();
        let job_key: String = Sha256::digest(b"job-three")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        for (index, source) in sources.iter().enumerate() {
            let filename = format!("kronn-{job_key}-{index}.png");
            Mock::given(method("POST"))
                .and(path("/upload/image"))
                .and(body_string_contains(format!("filename=\"{filename}\"")))
                .and(body_string_contains(
                    String::from_utf8(source.bytes.clone()).unwrap(),
                ))
                .and(body_string_contains("name=\"overwrite\"\r\n\r\ntrue"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"name":filename,"type":"input","subfolder":""}),
                ))
                .expect(2)
                .mount(&server)
                .await;
        }
        let client = reqwest::Client::new();
        let first = upload_comfyui_references(&client, &server.uri(), "job-three", &sources)
            .await
            .unwrap();
        let retry = upload_comfyui_references(&client, &server.uri(), "job-three", &sources)
            .await
            .unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.len(), 3);
        assert!(first[0].ends_with("-0.png") && first[2].ends_with("-2.png"));
        let other = MockServer::start().await;
        Mock::given(path("/upload/image"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"name":"wrong.png","type":"output"})),
            )
            .mount(&other)
            .await;
        assert!(
            upload_comfyui_references(&client, &other.uri(), "job-three", &sources)
                .await
                .unwrap_err()
                .to_string()
                .contains("unexpected")
        );
    }
    use crate::db::media_jobs::{self, NewMediaJob};
    use crate::models::{MediaJobStatus, MediaModality, MediaParams};
    use chrono::Duration as ChronoDuration;

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&Utc)
    }

    async fn db_with_job(id: &str, handle: Option<&str>, scheduled: DateTime<Utc>) -> Database {
        let db = Database::open_in_memory().expect("in-memory db");
        let id = id.to_string();
        let handle = handle.map(str::to_string);
        db.with_conn(move |conn| {
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: &id,
                    modality: MediaModality::Video,
                    connection_id: "conn-1",
                    model: "bytedance/seedance-2.0-mini",
                    prompt: "un chat",
                    params: &MediaParams::default(),
                    discussion_id: None,
                    message_id: None,
                    project_id: None,
                    scheduled_at: scheduled,
                    deadline_at: scheduled + ChronoDuration::minutes(20),
                },
                scheduled,
            )?;
            media_jobs::claim(conn, &id, scheduled)?;
            if let Some(handle) = handle.as_deref() {
                media_jobs::record_submission(conn, &id, handle, scheduled)?;
            }
            Ok(())
        })
        .await
        .expect("seed");
        db
    }

    /// KT-551 — the source image is re-read on every attempt, from the id the
    /// job stored, so a restart resumes with the same picture and a file that
    /// left the room fails the job instead of reaching the provider.
    #[tokio::test]
    async fn a_source_image_is_re_read_from_its_id_at_each_attempt() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("frame.png");
        tokio::fs::write(&path, b"first-bytes")
            .await
            .expect("write");

        let db = Database::open_in_memory().expect("in-memory db");
        let disk_path = path.to_string_lossy().to_string();
        db.with_conn({
            let disk_path = disk_path.clone();
            move |conn| {
                let now = Utc::now().to_rfc3339();
                conn.execute(
                    "INSERT INTO discussions (id, title, created_at, updated_at)
                     VALUES ('disc-1', 'Room', ?1, ?1)",
                    [&now],
                )?;
                conn.execute(
                    "INSERT INTO context_files
                        (id, discussion_id, filename, mime_type, original_size,
                         extracted_size, extracted_text, disk_path, created_at)
                     VALUES ('asset-1', 'disc-1', 'frame.png', 'image/png', 11, 0, '', ?1, ?2)",
                    rusqlite::params![disk_path, now],
                )?;
                Ok(())
            }
        })
        .await
        .expect("seed");

        let params = MediaParams {
            reference_asset_id: Some("asset-1".into()),
            reference_mode: Some(crate::models::MediaReferenceMode::FirstFrame),
            ..MediaParams::default()
        };
        let scheduled = at("2026-09-02T04:00:00Z");
        let job_id = "job-ref";
        db.with_conn({
            let params = params.clone();
            move |conn| {
                media_jobs::insert(
                    conn,
                    NewMediaJob {
                        id: job_id,
                        modality: MediaModality::Video,
                        connection_id: "conn-1",
                        model: "m",
                        prompt: "un renard",
                        params: &params,
                        discussion_id: Some("disc-1"),
                        message_id: None,
                        project_id: None,
                        scheduled_at: scheduled,
                        deadline_at: scheduled + ChronoDuration::minutes(20),
                    },
                    scheduled,
                )
            }
        })
        .await
        .expect("insert");
        let job = db
            .with_read_conn(move |conn| media_jobs::get(conn, job_id))
            .await
            .expect("read")
            .expect("job");

        let loaded = load_references(&db, &job).await.expect("readable");
        let first = loaded.first().expect("a source image");
        assert_eq!(first.bytes, b"first-bytes");
        assert_eq!(first.mime_type, "image/png");

        // The bytes are read now, not at launch: replacing the file changes
        // what the next attempt submits, which is what makes a restart resume
        // with the real current picture.
        tokio::fs::write(&path, b"second").await.expect("rewrite");
        let second = load_references(&db, &job).await.expect("readable");
        assert_eq!(second[0].bytes, b"second");

        // A file that disappeared fails the job with a sentence instead of
        // submitting a generation silently stripped of its source image.
        tokio::fs::remove_file(&path).await.expect("remove");
        let error = load_references(&db, &job).await.unwrap_err().to_string();
        assert!(error.contains("could not be read"), "got {error}");
    }

    #[tokio::test]
    async fn a_job_without_a_source_image_reads_nothing() {
        let db = db_with_job("job-plain", None, at("2026-09-02T04:00:00Z")).await;
        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "job-plain"))
            .await
            .expect("read")
            .expect("job");
        assert!(load_references(&db, &job).await.expect("ok").is_empty());
    }

    /// One-request HTTP server, returning the address and the raw request head
    /// it received. Asserting on that head is the only way to know what
    /// actually left the process — a unit test on the policy cannot.
    async fn one_shot(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("local addr").to_string();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("one connection");
            let mut seen = Vec::new();
            let mut buf = [0u8; 1024];
            // Read until the head is complete; the body (if any) is not needed
            // by these assertions.
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => seen.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            // Drain an announced body before answering: replying mid-upload
            // can reset the connection before the client reads the response.
            if let Some(len) = content_length(&seen) {
                let head_end = seen
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|i| i + 4)
                    .unwrap_or(seen.len());
                let mut body = seen.len().saturating_sub(head_end);
                while body < len {
                    match socket.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => body += n,
                        Err(_) => break,
                    }
                }
            }
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
            String::from_utf8_lossy(&seen).to_string()
        });
        (addr, handle)
    }

    fn content_length(head: &[u8]) -> Option<usize> {
        String::from_utf8_lossy(head)
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:").or(line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|_| line.split(':').nth(1).unwrap_or_default()))
            })
            .and_then(|value| value.trim().parse().ok())
    }

    #[tokio::test]
    async fn a_pre_signed_asset_url_never_receives_the_provider_credential() {
        const SECRET: &str = "sk-or-v1-must-never-leave";
        let (addr, captured) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").await;
        let bytes = download_asset(
            &ValidatedAssetUrl {
                url: format!("http://{addr}/asset.mp4"),
                send_credential: false,
                on_configured_origin: false,
            },
            SECRET,
            crate::core::safe_http::SafeHttpPolicy::PublicOrLoopbackForTests,
        )
        .await
        .expect("an anonymous download must still work");
        assert_eq!(bytes, b"abc");

        let head = captured.await.expect("server task");
        assert!(
            !head.to_ascii_lowercase().contains("authorization"),
            "no Authorization header may reach a host that did not earn it: {head}"
        );
        assert!(
            !head.contains(SECRET),
            "the credential itself must not appear anywhere in the request"
        );
    }

    #[tokio::test]
    async fn a_credentialed_asset_url_does_carry_it_so_the_check_above_can_fail() {
        // Guards the test above: if the header were never sent in either case,
        // its assertion would pass while proving nothing.
        const SECRET: &str = "sk-or-v1-expected-here";
        let (addr, captured) = one_shot("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc").await;
        download_asset(
            &ValidatedAssetUrl {
                url: format!("http://{addr}/asset.mp4"),
                send_credential: true,
                on_configured_origin: true,
            },
            SECRET,
            DOWNLOAD_PUBLIC,
        )
        .await
        .expect("download");
        let head = captured.await.expect("server task");
        assert!(head.contains(SECRET), "expected the bearer here: {head}");
    }

    #[tokio::test]
    async fn an_asset_redirect_to_an_internal_address_is_refused() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(path("/asset.mp4"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;
        for on_configured_origin in [true, false] {
            let err = download_asset(
                &ValidatedAssetUrl {
                    url: format!("{}/asset.mp4", server.uri()),
                    send_credential: on_configured_origin,
                    on_configured_origin,
                },
                "sk-test",
                crate::core::safe_http::SafeHttpPolicy::PublicOrLoopbackForTests,
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("Security"), "{err}");
        }
    }

    /// A synchronous image response carrying a real 1x1 PNG and a billed cost.
    const IMAGE_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 176\r\n\r\n{\"id\":\"gen-img-42\",\"data\":[{\"b64_json\":\"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8AAAwAB/wD/2v0AAAAASUVORK5CYII=\"}],\"usage\":{\"cost\":0.0123,\"is_byok\":false}}";

    #[tokio::test]
    #[serial_test::serial] // KRONN_DATA_DIR is process-wide
    async fn an_image_generation_persists_its_cost_and_anchors_its_asset() {
        let scratch = std::env::temp_dir().join(format!("kronn-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
        crate::core::child_env::set_var("KRONN_DATA_DIR", &scratch);

        let now = at("2026-09-01T10:00:00Z");
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('d-1', 'Media', ?1, ?1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) \
                 VALUES ('m-launch', 'd-1', 'User', 'un chat en origami', ?1, 1)",
                ["2026-09-01T09:00:00Z"],
            )?;
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: "job-img",
                    modality: MediaModality::Image,
                    connection_id: "conn-1",
                    model: "google/gemini-2.5-flash-image",
                    prompt: "un chat en origami",
                    params: &MediaParams::default(),
                    discussion_id: Some("d-1"),
                    message_id: Some("m-launch"),
                    project_id: None,
                    scheduled_at: now,
                    deadline_at: now + ChronoDuration::minutes(20),
                },
                now,
            )?;
            Ok(())
        })
        .await
        .expect("seed");

        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "job-img"))
            .await
            .expect("read")
            .expect("job");

        let (addr, _server) = one_shot(IMAGE_RESPONSE).await;
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();
        let status = advance(
            &db,
            MediaContext {
                codec: &crate::agents::media_codec::OpenRouterMediaCodec,
                base: &base,
                api_key: "sk-test",
                client: &client,
            },
            &job,
            now,
        )
        .await
        .expect("the image path must complete against a real response");
        assert_eq!(status, MediaJobStatus::Completed);

        let settled = db
            .with_read_conn(|conn| media_jobs::get(conn, "job-img"))
            .await
            .expect("read")
            .expect("job");
        // Images are billed too: the figure exists only in that response.
        assert_eq!(
            settled.cost,
            Some(crate::models::MediaCost {
                cost_usd: 0.0123,
                is_byok: false
            })
        );
        assert_eq!(
            settled.provider_generation_id.as_deref(),
            Some("gen-img-42")
        );
        // Geometry read from the produced bytes, not from the request.
        assert_eq!(settled.rendered.width, Some(1));
        assert_eq!(settled.rendered.height, Some(1));

        let file_id = settled.context_file_id.expect("an asset was stored");
        let anchored: Option<String> = db
            .with_read_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT message_id FROM context_files WHERE id = ?1",
                    [&file_id],
                    |row| row.get(0),
                )?)
            })
            .await
            .expect("read the stored file");
        // The whole point of blocker 4: never left pending, or the next human
        // message would claim the asset as its own attachment.
        assert_eq!(
            anchored.as_deref(),
            Some("m-launch"),
            "a generated asset must hang from the turn that asked for it"
        );
        // KT-1202 — without a project the asset still falls back to the data dir.
        let disk_path: String = db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT disk_path FROM context_files WHERE discussion_id = 'd-1'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .expect("stored path");
        assert!(
            std::path::Path::new(&disk_path).starts_with(&scratch),
            "{disk_path} must fall back to the data dir"
        );

        match previous {
            Some(value) => crate::core::child_env::set_var("KRONN_DATA_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }

    /// KT-1202 — a generated asset is kept in the project's main checkout, so
    /// removing the worktree never takes it away, and mirrored in the worktree
    /// so the agent running there can open it.
    #[tokio::test]
    async fn a_generated_asset_survives_its_worktree_and_is_mirrored_for_the_agent() {
        let root = tempfile::tempdir().expect("temp dir");
        let project = root.path().join("project");
        let worktree = root.path().join("worktree");
        std::fs::create_dir_all(&project).expect("project dir");
        std::fs::create_dir_all(&worktree).expect("worktree dir");
        let now = at("2026-09-01T10:00:00Z");
        let db = Database::open_in_memory().expect("in-memory db");
        let (project_path, worktree_path) = (
            project.to_string_lossy().to_string(),
            worktree.to_string_lossy().to_string(),
        );
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at) \
                 VALUES ('p-1', 'P', ?1, ?2, ?2)",
                [&project_path, "2026-09-01T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO discussions (id, title, project_id, workspace_path, created_at, updated_at) \
                 VALUES ('d-1', 'Media', 'p-1', ?1, ?2, ?2)",
                [&worktree_path, "2026-09-01T09:00:00Z"],
            )?;
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: "job-img",
                    modality: MediaModality::Image,
                    connection_id: "conn-1",
                    model: "google/gemini-2.5-flash-image",
                    prompt: "un chat",
                    params: &MediaParams::default(),
                    discussion_id: Some("d-1"),
                    message_id: None,
                    project_id: Some("p-1"),
                    scheduled_at: now,
                    deadline_at: now + ChronoDuration::minutes(20),
                },
                now,
            )?;
            media_jobs::claim(conn, "job-img", now)?;
            Ok(())
        })
        .await
        .expect("seed");
        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "job-img"))
            .await
            .expect("read")
            .expect("job");

        let client = reqwest::Client::new();
        let ctx = MediaContext {
            codec: &crate::agents::media_codec::OpenRouterMediaCodec,
            base: "http://unused.invalid",
            api_key: "sk-test",
            client: &client,
        };
        persist(&db, ctx, &job, b"png-bytes".to_vec(), None, None, now)
            .await
            .expect("persist");

        let disk_path: String = db
            .with_read_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT disk_path FROM context_files WHERE discussion_id = 'd-1'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .await
            .expect("stored row");
        let stored = std::path::Path::new(&disk_path);
        assert!(
            stored.starts_with(project.join(".kronn").join("context-files")),
            "{disk_path} must sit in the project's main checkout"
        );
        let mirror = worktree
            .join(".kronn")
            .join("context-files")
            .join(stored.file_name().expect("name"));
        assert_eq!(std::fs::read(&mirror).expect("agent mirror"), b"png-bytes");

        // The route serves `disk_path`: it outlives the worktree.
        std::fs::remove_dir_all(&worktree).expect("remove worktree");
        assert_eq!(std::fs::read(stored).expect("still served"), b"png-bytes");
    }

    #[tokio::test]
    async fn a_transient_failure_puts_the_job_back_in_the_queue_without_a_restart() {
        // The whole point: `due()` only picks up `pending`, so a job left
        // `running` after a network blip is frozen until a restart — on a
        // generation the provider is already billing.
        let now = at("2026-09-01T12:00:00Z");
        let db = db_with_job("j1", Some("provider-abc"), now).await;

        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "j1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, MediaJobStatus::Running);

        settle_failed_advance(
            &db,
            &job,
            &AdvanceFailure::Transient("connection reset".into()),
            now,
        )
        .await
        .unwrap();

        let after = db
            .with_read_conn(|conn| media_jobs::get(conn, "j1"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, MediaJobStatus::Pending, "must be retryable");
        // The provider handle survives, so the retry polls instead of paying
        // for a second generation.
        assert_eq!(after.provider_job_id.as_deref(), Some("provider-abc"));

        // And it really comes back through the normal due path, no restart.
        let later = now + ChronoDuration::minutes(1);
        let due = db
            .with_read_conn(move |conn| media_jobs::due(conn, later, 10))
            .await
            .unwrap();
        assert!(due.iter().any(|j| j.id == "j1"), "job never re-queued");
    }

    #[tokio::test]
    async fn a_permanent_failure_settles_the_job_with_an_actionable_diagnostic() {
        let now = at("2026-09-01T12:00:00Z");
        let db = db_with_job("j2", None, now).await;
        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "j2"))
            .await
            .unwrap()
            .unwrap();

        settle_failed_advance(
            &db,
            &job,
            &AdvanceFailure::Permanent(
                "this connection has no stored credential — add its API key".into(),
            ),
            now,
        )
        .await
        .unwrap();

        let after = db
            .with_read_conn(|conn| media_jobs::get(conn, "j2"))
            .await
            .unwrap()
            .unwrap();
        // Retrying a missing credential cannot help, so burning the deadline
        // on it would only delay the same answer.
        assert_eq!(after.status, MediaJobStatus::Failed);
        let error = after.last_error.unwrap_or_default();
        assert!(error.contains("credential"), "not actionable: {error}");

        let due = db
            .with_read_conn(move |conn| media_jobs::due(conn, now, 10))
            .await
            .unwrap();
        assert!(
            due.is_empty(),
            "a permanently failed job must not be retried"
        );
    }

    #[tokio::test]
    async fn an_overdue_job_is_settled_by_the_deadline_not_left_running() {
        let started = at("2026-09-01T12:00:00Z");
        let db = db_with_job("j3", Some("provider-abc"), started).await;

        // Well past the 20-minute budget.
        let later = started + ChronoDuration::hours(2);
        let expired = db
            .with_conn(move |conn| media_jobs::expire_overdue(conn, later))
            .await
            .unwrap();
        // The ids come back so the caller can publish them; a count could not.
        assert_eq!(expired, vec!["j3".to_string()]);

        let after = db
            .with_read_conn(|conn| media_jobs::get(conn, "j3"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, MediaJobStatus::TimedOut);
        assert_eq!(after.last_error.as_deref(), Some("deadline exceeded"));

        // Publication is deliberately NOT asserted here: recreating the
        // projection by hand would prove the mapper, not the loop. The real
        // sweep — expiry plus a targeted SharedRunUpdated — is exercised in
        // `api_tests::an_expired_media_job_broadcasts_through_the_real_worker_sweep`.
    }

    #[tokio::test]
    async fn the_deadline_still_wins_over_a_retryable_failure() {
        // A transient error must not resurrect a job past its budget: the
        // reschedule puts it back in the queue, and the next sweep expires it.
        let started = at("2026-09-01T12:00:00Z");
        let db = db_with_job("j4", Some("provider-abc"), started).await;
        let job = db
            .with_read_conn(|conn| media_jobs::get(conn, "j4"))
            .await
            .unwrap()
            .unwrap();

        let later = started + ChronoDuration::hours(2);
        settle_failed_advance(
            &db,
            &job,
            &AdvanceFailure::Transient("timeout".into()),
            later,
        )
        .await
        .unwrap();
        let expired = db
            .with_conn(move |conn| media_jobs::expire_overdue(conn, later))
            .await
            .unwrap();
        assert_eq!(expired, vec!["j4".to_string()]);

        let after = db
            .with_read_conn(|conn| media_jobs::get(conn, "j4"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, MediaJobStatus::TimedOut);
    }

    /// KT-1381 — a local ComfyUI image is queued like a video: the submission
    /// records ComfyUI's prompt id, a poll reads `/history`, and the file is
    /// fetched from `/view` on the same origin and stored with no cost.
    #[tokio::test]
    #[serial_test::serial] // KRONN_DATA_DIR is process-wide
    async fn a_local_comfyui_image_is_queued_polled_and_stored_without_a_cost() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let scratch_dir = tempfile::tempdir().expect("scratch dir");
        let scratch = scratch_dir.path().to_path_buf();
        let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
        crate::core::child_env::set_var("KRONN_DATA_DIR", &scratch);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "prompt_id": "p-42", "number": 1, "node_errors": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/p-42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "p-42": {
                    "outputs": {"7": {"images": [{"filename": "kronn_00001_.png", "subfolder": "", "type": "output"}]}},
                    "status": {"status_str": "success", "completed": true, "messages": []}
                }
            })))
            .mount(&server)
            .await;
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8AAAwAB/wD/2v0AAAAASUVORK5CYII=")
            .unwrap();
        Mock::given(method("GET"))
            .and(path("/view"))
            .and(query_param("filename", "kronn_00001_.png"))
            .and(query_param("type", "output"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png))
            .expect(1)
            .mount(&server)
            .await;

        let now = at("2026-10-19T10:00:00Z");
        let db = Database::open_in_memory().expect("in-memory db");
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO discussions (id, title, created_at, updated_at) \
                 VALUES ('d-1', 'Media', ?1, ?1)",
                ["2026-10-19T09:00:00Z"],
            )?;
            conn.execute(
                "INSERT INTO messages (id, discussion_id, role, content, timestamp, sort_order) \
                 VALUES ('m-launch', 'd-1', 'User', 'un renard', ?1, 1)",
                ["2026-10-19T09:00:00Z"],
            )?;
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id: "job-local",
                    modality: MediaModality::Image,
                    connection_id: crate::core::comfyui::CONNECTION_ID,
                    model: "RealVisXL_V5.0_fp16.safetensors",
                    prompt: "un renard",
                    params: &MediaParams::default(),
                    discussion_id: Some("d-1"),
                    message_id: Some("m-launch"),
                    project_id: None,
                    scheduled_at: now,
                    deadline_at: now + ChronoDuration::minutes(20),
                },
                now,
            )?;
            Ok(())
        })
        .await
        .expect("seed");

        let base = server.uri();
        let codec = crate::agents::media_codec::ComfyUiMediaCodec {
            base: base.clone(),
            seed: 7,
            image_family: Some(crate::core::comfyui::ComfyFamily::Sdxl),
            settings: crate::core::comfyui::ComfyFamily::Sdxl.defaults(),
            video_companions: None,
            video_family: None,
            source_images: Vec::new(),
            denoise: 0.55,
        };
        let client = reqwest::Client::new();
        async fn read(db: &Database, id: &'static str) -> MediaJob {
            db.with_read_conn(move |conn| media_jobs::get(conn, id))
                .await
                .expect("read")
                .expect("job")
        }
        let ctx = || MediaContext {
            codec: &codec,
            base: &base,
            api_key: "",
            client: &client,
        };

        let submitted = advance(&db, ctx(), &read(&db, "job-local").await, now)
            .await
            .expect("the submission is queued");
        assert_eq!(submitted, MediaJobStatus::Pending);
        let queued = read(&db, "job-local").await;
        assert_eq!(queued.provider_job_id.as_deref(), Some("p-42"));

        let done = advance(&db, ctx(), &queued, now)
            .await
            .expect("the poll completes the job");
        assert_eq!(done, MediaJobStatus::Completed);
        let settled = read(&db, "job-local").await;
        // Local generation bills nothing and must never feed an estimate.
        assert_eq!(settled.cost, None);
        assert_eq!(settled.rendered.width, Some(1));
        assert!(settled.context_file_id.is_some(), "the image was stored");

        match previous {
            Some(value) => crate::core::child_env::set_var("KRONN_DATA_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
        }
        drop(scratch_dir);
    }

    fn local_state(endpoint: &str) -> crate::AppState {
        use std::sync::Arc;
        let db = Arc::new(Database::open_in_memory().expect("in-memory db"));
        let mut config = crate::core::config::default_config();
        config.comfyui.endpoint = Some(endpoint.to_string());
        config.comfyui.image_model = Some("RealVisXL_V5.0_fp16.safetensors".into());
        crate::AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(config)),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        )
    }

    async fn seed_local_job(
        db: &Database,
        id: &'static str,
        now: DateTime<Utc>,
        deadline: DateTime<Utc>,
        handle: Option<&'static str>,
    ) -> MediaJob {
        db.with_conn(move |conn| {
            media_jobs::insert(
                conn,
                NewMediaJob {
                    id,
                    modality: MediaModality::Image,
                    connection_id: crate::core::comfyui::CONNECTION_ID,
                    model: "RealVisXL_V5.0_fp16.safetensors",
                    prompt: "un renard",
                    params: &MediaParams::default(),
                    discussion_id: None,
                    message_id: None,
                    project_id: None,
                    scheduled_at: now,
                    deadline_at: deadline,
                },
                now,
            )?;
            if let Some(handle) = handle {
                media_jobs::mark_submit_attempt(conn, id, now)?;
                media_jobs::record_submission(conn, id, handle, now)?;
            }
            Ok(())
        })
        .await
        .expect("seed");
        db.with_read_conn(move |conn| media_jobs::get(conn, id))
            .await
            .expect("read")
            .expect("job")
    }

    /// Review P2 — a stopped ComfyUI is told at once, not retried for 20 min,
    /// and a job it no longer knows fails instead of waiting for the deadline.
    #[tokio::test]
    async fn a_stopped_comfyui_or_a_lost_job_fails_with_a_sentence_at_once() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let now = at("2026-10-20T10:00:00Z");
        let client = reqwest::Client::new();

        let down = local_state("http://127.0.0.1:9");
        let job = seed_local_job(
            &down.db,
            "job-down",
            now,
            now + ChronoDuration::minutes(20),
            None,
        )
        .await;
        match advance_claimed(&down, &client, &job, now).await {
            Err(AdvanceFailure::Permanent(message)) => {
                assert!(message.contains("does not answer"), "{message}")
            }
            other => panic!("expected a permanent refusal, got {other:?}"),
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/history/p-lost"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "queue_running": [[3, "someone-else", {}, {}, []]], "queue_pending": []
            })))
            .mount(&server)
            .await;
        let restarted = local_state(&server.uri());
        let job = seed_local_job(
            &restarted.db,
            "job-lost",
            now,
            now + ChronoDuration::minutes(20),
            Some("p-lost"),
        )
        .await;
        match advance_claimed(&restarted, &client, &job, now).await {
            Err(AdvanceFailure::Permanent(message)) => {
                assert!(message.contains("no longer knows"), "{message}")
            }
            other => panic!("expected a permanent failure, got {other:?}"),
        }
    }

    /// A slot chosen before its family was saved with it: the first job
    /// reads the family and saves it, so later jobs never depend on the
    /// header again.
    #[tokio::test]
    #[serial_test::serial] // KRONN_DATA_DIR is process-wide
    async fn the_first_local_job_saves_the_family_an_older_slot_lacked() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let scratch = tempfile::tempdir().expect("scratch dir");
        let previous = crate::core::child_env::var_os("KRONN_DATA_DIR");
        crate::core::child_env::set_var("KRONN_DATA_DIR", scratch.path());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/system_stats"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"system": {"comfyui_version": "0.42.1"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"prompt_id": "p-1"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let state = local_state(&server.uri());
        assert_eq!(state.config.read().await.comfyui.image_family, None);
        let now = at("2026-10-21T10:00:00Z");
        let job = seed_local_job(
            &state.db,
            "job-old-slot",
            now,
            now + ChronoDuration::minutes(20),
            None,
        )
        .await;
        advance_claimed(&state, &reqwest::Client::new(), &job, now)
            .await
            .expect("submitted");

        assert_eq!(
            state.config.read().await.comfyui.image_family,
            Some(crate::core::comfyui::ComfyFamily::Sdxl)
        );
        let saved = std::fs::read_to_string(scratch.path().join("config.toml")).unwrap_or_default();
        assert!(saved.contains("image_family"), "{saved}");

        match previous {
            Some(value) => crate::core::child_env::set_var("KRONN_DATA_DIR", value),
            None => crate::core::child_env::remove_var("KRONN_DATA_DIR"),
        }
    }

    /// A finished job with no saved file fails on that poll.
    #[tokio::test]
    async fn a_comfyui_job_finished_without_an_output_fails_on_that_poll() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/history/p-empty"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "p-empty": { "outputs": {}, "status": {"status_str": "success", "completed": true, "messages": []} }
            })))
            .mount(&server)
            .await;
        let now = at("2026-10-20T10:00:00Z");
        let state = local_state(&server.uri());
        let job = seed_local_job(
            &state.db,
            "job-empty",
            now,
            now + ChronoDuration::minutes(20),
            Some("p-empty"),
        )
        .await;
        advance_claimed(&state, &reqwest::Client::new(), &job, now)
            .await
            .expect("the poll settles the job");
        let settled = state
            .db
            .with_read_conn(|conn| media_jobs::get(conn, "job-empty"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, MediaJobStatus::Failed);
        assert!(settled
            .last_error
            .unwrap_or_default()
            .contains("no output file"));
    }

    /// A lost handle on the local source is simply submitted again: nothing
    /// is billed twice there.
    #[tokio::test]
    async fn a_local_job_whose_handle_was_lost_is_submitted_again() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"prompt_id": "p-again"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let now = at("2026-10-20T10:00:00Z");
        let db = Database::open_in_memory().unwrap();
        let mut job = seed_local_job(
            &db,
            "job-again",
            now,
            now + ChronoDuration::minutes(20),
            None,
        )
        .await;
        db.with_conn(|conn| {
            media_jobs::mark_submit_attempt(conn, "job-again", at("2026-10-20T09:59:00Z"))?;
            Ok(())
        })
        .await
        .unwrap();
        job.submit_attempted_at = Some(at("2026-10-20T09:59:00Z"));
        let base = server.uri();
        let codec = crate::agents::media_codec::ComfyUiMediaCodec {
            base: base.clone(),
            seed: 1,
            image_family: Some(crate::core::comfyui::ComfyFamily::Sdxl),
            settings: crate::core::comfyui::ComfyFamily::Sdxl.defaults(),
            video_companions: None,
            video_family: None,
            source_images: Vec::new(),
            denoise: 0.55,
        };
        let client = reqwest::Client::new();
        let ctx = MediaContext {
            codec: &codec,
            base: &base,
            api_key: "",
            client: &client,
        };
        assert_eq!(
            advance(&db, ctx, &job, now).await.unwrap(),
            MediaJobStatus::Pending
        );
        let resubmitted = db
            .with_read_conn(|conn| media_jobs::get(conn, "job-again"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resubmitted.provider_job_id.as_deref(), Some("p-again"));
    }

    /// Review P2 — a job that times out is cancelled on ComfyUI by its id.
    #[tokio::test]
    async fn an_expired_local_job_is_cancelled_on_comfyui_by_its_id() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/jobs/p-slow/cancel"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"cancelled": true})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/interrupt"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let state = local_state(&server.uri());
        let past = Utc::now() - ChronoDuration::hours(3);
        seed_local_job(
            &state.db,
            "job-slow",
            past,
            past + ChronoDuration::minutes(90),
            Some("p-slow"),
        )
        .await;
        tick(&state, &reqwest::Client::new()).await.expect("tick");
        let settled = state
            .db
            .with_read_conn(|conn| media_jobs::get(conn, "job-slow"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, MediaJobStatus::TimedOut);
    }

    /// On a ComfyUI without cancel-by-id: a queued prompt is deleted by its
    /// id, a rendering one is never interrupted globally — the job says so —
    /// and an unknown one is left alone.
    #[tokio::test]
    async fn an_older_comfyui_is_never_interrupted_globally() {
        use wiremock::matchers::{body_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for (queue, delete_calls, note) in [
            (
                serde_json::json!({"queue_running": [], "queue_pending": [[2, "p-mine", {}, {}, []]]}),
                1,
                false,
            ),
            (
                serde_json::json!({"queue_running": [[1, "p-mine", {}, {}, []]], "queue_pending": []}),
                0,
                true,
            ),
            (
                serde_json::json!({"queue_running": [[1, "someone-else", {}, {}, []]], "queue_pending": []}),
                0,
                false,
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/api/jobs/p-mine/cancel"))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/queue"))
                .respond_with(ResponseTemplate::new(200).set_body_json(queue))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/queue"))
                .and(body_json(serde_json::json!({"delete": ["p-mine"]})))
                .respond_with(ResponseTemplate::new(200))
                .expect(delete_calls)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/interrupt"))
                .respond_with(ResponseTemplate::new(200))
                .expect(0)
                .mount(&server)
                .await;
            let state = local_state(&server.uri());
            let now = Utc::now();
            seed_local_job(
                &state.db,
                "job-old",
                now,
                now + ChronoDuration::minutes(90),
                Some("p-mine"),
            )
            .await;
            state
                .db
                .with_conn(move |conn| media_jobs::cancel(conn, "job-old", now))
                .await
                .unwrap();
            crate::api::comfyui::release_local_job(&state, "job-old").await;
            let job = state
                .db
                .with_read_conn(|conn| media_jobs::get(conn, "job-old"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.status, MediaJobStatus::Cancelled);
            assert_eq!(
                job.last_error
                    .unwrap_or_default()
                    .contains("finishes rendering"),
                note
            );
            server.verify().await;
        }
    }
}
