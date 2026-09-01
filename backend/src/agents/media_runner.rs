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
        MediaModality::Image => {
            let url = ctx.codec.image_url(ctx.base);
            let body = ctx.codec.image_body(&job.model, &job.prompt, &job.params);
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
        MediaModality::Video => {
            let url = ctx.codec.video_submit_url(ctx.base);
            let body = ctx.codec.video_body(&job.model, &job.prompt, &job.params);
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
            reschedule(db, &job.id, job.attempts, now).await?;
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
            reschedule(db, &job.id, job.attempts, now).await?;
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
            let bytes = download_asset(ctx.client, &cleared, ctx.api_key).await?;
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
            reschedule(db, &job.id, job.attempts, now).await?;
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
    let disk_path = crate::core::context_files::save_file_to_disk(&file_id, &filename, &bytes)?;

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

async fn reschedule(db: &Database, job_id: &str, attempts: u32, now: DateTime<Utc>) -> Result<()> {
    let delay = backoff_delay(attempts);
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

/// Download of the finished asset. Server-side on purpose: the provider URL
/// still requires the credential, and must never reach a browser.
async fn download_asset(
    client: &reqwest::Client,
    cleared: &ValidatedAssetUrl,
    api_key: &str,
) -> Result<Vec<u8>> {
    let request = client.get(&cleared.url).timeout(Duration::from_secs(300));
    // A pre-signed URL is its own authorisation: attaching the key there would
    // hand it to whoever serves that host.
    let request = if cleared.send_credential {
        request.bearer_auth(api_key)
    } else {
        request
    };
    let response = request.send().await?;
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
        if let Err(e) = crate::api::shared_runs::publish_media_job(state, job_id).await {
            tracing::warn!(job = %job_id, error = %e, "expired media run publication failed");
        }
    }

    let due = state
        .db
        .with_read_conn(move |conn| crate::db::media_jobs::due(conn, now, BATCH))
        .await?;

    for job in due {
        let id = job.id.clone();
        let claimed = state
            .db
            .with_conn(move |conn| crate::db::media_jobs::claim(conn, &id, now))
            .await?;
        if !claimed {
            // Another worker took it; never run a billed generation twice.
            continue;
        }
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
    let codec: Box<dyn crate::agents::media_codec::MediaCodec> = match connection.origin_preset {
        crate::models::ExternalApiConnectionPreset::Nvidia => {
            Box::new(crate::agents::media_codec::NvidiaMediaCodec)
        }
        _ => Box::new(crate::agents::media_codec::OpenRouterMediaCodec),
    };
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

#[cfg(test)]
mod tests {
    use super::*;
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
            &reqwest::Client::new(),
            &ValidatedAssetUrl {
                url: format!("http://{addr}/asset.mp4"),
                send_credential: false,
            },
            SECRET,
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
            &reqwest::Client::new(),
            &ValidatedAssetUrl {
                url: format!("http://{addr}/asset.mp4"),
                send_credential: true,
            },
            SECRET,
        )
        .await
        .expect("download");
        let head = captured.await.expect("server task");
        assert!(head.contains(SECRET), "expected the bearer here: {head}");
    }

    /// A synchronous image response carrying a real 1x1 PNG and a billed cost.
    const IMAGE_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 176\r\n\r\n{\"id\":\"gen-img-42\",\"data\":[{\"b64_json\":\"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8AAAwAB/wD/2v0AAAAASUVORK5CYII=\"}],\"usage\":{\"cost\":0.0123,\"is_byok\":false}}";

    #[tokio::test]
    #[serial_test::serial] // KRONN_DATA_DIR is process-wide
    async fn an_image_generation_persists_its_cost_and_anchors_its_asset() {
        let scratch = std::env::temp_dir().join(format!("kronn-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let previous = std::env::var_os("KRONN_DATA_DIR");
        std::env::set_var("KRONN_DATA_DIR", &scratch);

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

        match previous {
            Some(value) => std::env::set_var("KRONN_DATA_DIR", value),
            None => std::env::remove_var("KRONN_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&scratch);
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
}
