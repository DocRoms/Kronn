//! Ollama local LLM endpoints (v0.4.0 — Phase 1).
//!
//! Health checks, model listing and agent execution use Ollama's HTTP API.
//!
//! Ollama runs on the HOST machine (not in the Docker container).
//! In Docker, we reach it via `host.docker.internal:11434`.

use crate::models::*;
use crate::AppState;
use axum::{
    extract::{Query, State},
    response::sse::{Event, Sse},
    Json,
};
use futures::{Stream, StreamExt};
use std::{convert::Infallible, pin::Pin, time::Duration};

type OllamaSseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// Resolve the endpoint supplied by the runner, falling back to Ollama's
/// process-level configuration when no invocation-local endpoint was given.
///
/// Production currently supplies `None`; the explicit branch exists so an
/// isolated invocation (notably a test mock) never has to mutate `OLLAMA_HOST`.
pub fn resolve_base_url_pub(explicit: Option<&str>) -> String {
    explicit.map(str::to_owned).unwrap_or_else(ollama_base_url)
}

/// Resolve the Ollama API base URL.
/// Priority: OLLAMA_HOST env var > Docker heuristic > localhost.
fn ollama_base_url() -> String {
    if let Ok(host) = crate::core::child_env::var("OLLAMA_HOST") {
        if !host.is_empty() && !crate::core::net_expose::is_exposed_host(&host) {
            if host.starts_with("http://") || host.starts_with("https://") {
                return host;
            }
            return format!("http://{}", host);
        }
    }
    if crate::core::env::is_docker() {
        "http://host.docker.internal:11434".to_string()
    } else {
        "http://localhost:11434".to_string()
    }
}

/// Detect the host environment for contextual error messages.
fn detect_context() -> &'static str {
    if !crate::core::env::is_docker() {
        return "native";
    }
    // Inside Docker: check KRONN_HOST_OS to distinguish WSL/macOS/Linux
    match crate::core::child_env::var("KRONN_HOST_OS").as_deref() {
        Ok("WSL") => "docker_wsl",
        Ok("macOS") => "docker_macos",
        _ => "docker_linux",
    }
}

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// The running server's own version string, from `/api/version`. Asked fresh
/// on every health probe, unlike the per-endpoint cache the runner keeps: the
/// natural answer to "no MLX models offered" is to update Ollama, and the
/// Refresh button must see the new version without restarting Kronn.
///
/// Bounded tighter than the probe that precedes it: the server has just
/// answered `/api/tags`, and callers that wrap `health` in their own deadline
/// (the worker reachability check allows four seconds) must never lose a
/// reachable server to an optional extra.
async fn fetch_server_version(client: &reqwest::Client, base: &str) -> Option<String> {
    let body = client
        .get(format!("{}/api/version", base.trim_end_matches('/')))
        .timeout(VERSION_PROBE_TIMEOUT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<serde_json::Value>()
        .await
        .ok()?;
    version_from_body(&body)
}

/// `{"version": "0.34.2"}` → `"0.34.2"`; anything else is no answer.
fn version_from_body(body: &serde_json::Value) -> Option<String> {
    body["version"]
        .as_str()
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_owned)
}

/// KT-930 — whether `-mlx` models are worth offering on this host: a Mac on
/// Apple Silicon running an Ollama whose MLX engine Kronn has measured
/// behaving. The version floor is the one the runner already scopes its MLX
/// worker mitigations to (`mlx_prefix_cache_reused`, measured on 0.34.2):
/// below it the MLX engine lacks prompt-prefix reuse (ollama/ollama#17829),
/// which is not what a suggestion should steer a user towards. An unknown
/// version is not capable.
pub(crate) fn mlx_capable(apple_silicon: bool, version: Option<&str>) -> bool {
    apple_silicon
        && crate::agents::runner::mlx_prefix_cache_reused(
            version.and_then(crate::agents::runner::parse_ollama_version),
        )
}

/// The health answer for a reachable server, from facts already gathered.
fn online_health(
    endpoint: String,
    models_count: u32,
    version: Option<String>,
    apple_silicon: bool,
) -> OllamaHealthResponse {
    let hint = if models_count == 0 {
        Some(
            "Ollama est en ligne mais aucun modèle n'est installé. Exécutez : ollama pull qwen3:8b"
                .into(),
        )
    } else {
        None
    };
    OllamaHealthResponse {
        status: "online".into(),
        mlx_capable: mlx_capable(apple_silicon, version.as_deref()),
        version,
        endpoint,
        models_count,
        hint,
    }
}

/// GET /api/ollama/health
///
/// Probe Ollama availability with contextual error messages.
/// The `hint` field provides a user-friendly explanation adapted to the
/// detected environment (native, Docker on WSL, Docker on macOS, etc.).
/// An online answer also carries the server's version and `mlx_capable`.
pub async fn health(State(state): State<AppState>) -> Json<ApiResponse<OllamaHealthResponse>> {
    let base = resolve_base_url_pub(state.ollama_base_url_override.as_deref());
    let context = detect_context();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    // Try the HTTP API
    match client.get(format!("{}/api/tags", base)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let models_count = body["models"]
                .as_array()
                .map(|a| a.len() as u32)
                .unwrap_or(0);
            let version = fetch_server_version(&client, &base).await;

            Json(ApiResponse::ok(online_health(
                base,
                models_count,
                version,
                crate::core::env::host_is_apple_silicon(),
            )))
        }
        _ => {
            // HTTP failed — build contextual hint
            let has_binary = which::which("ollama").is_ok();

            let (status, hint) = match (context, has_binary) {
                // Native: binary found but server not running
                ("native", true) => (
                    "offline",
                    "Ollama est installé mais le serveur n'est pas lancé. Exécutez : ollama serve",
                ),
                // Native: not installed
                ("native", false) => (
                    "not_installed",
                    "Ollama n'est pas installé. Rendez-vous sur https://ollama.com pour l'installer.",
                ),
                // Docker on WSL: most common issue — Ollama listens on 127.0.0.1 only
                ("docker_wsl", _) => (
                    "unreachable",
                    "Ollama ne répond pas depuis le container Docker. Sur WSL, Ollama écoute par défaut sur 127.0.0.1 uniquement. Relancez-le avec :\nOLLAMA_HOST=0.0.0.0 ollama serve",
                ),
                // Docker on Linux: same issue
                ("docker_linux", _) => (
                    "unreachable",
                    "Ollama ne répond pas depuis le container Docker. Sur Linux, relancez Ollama avec :\nOLLAMA_HOST=0.0.0.0 ollama serve",
                ),
                // Docker on macOS: host.docker.internal should work
                ("docker_macos", _) => (
                    "unreachable",
                    "Ollama ne répond pas. Vérifiez qu'il est lancé sur votre Mac : ollama serve",
                ),
                // Fallback
                (_, _) => (
                    "offline",
                    "Ollama ne répond pas. Vérifiez qu'il est installé et lancé : ollama serve",
                ),
            };

            Json(ApiResponse::ok(OllamaHealthResponse {
                status: status.into(),
                version: None,
                endpoint: base,
                models_count: 0,
                hint: Some(hint.into()),
                mlx_capable: false,
            }))
        }
    }
}

/// GET /api/ollama/models
///
/// List locally installed Ollama models. Uses the HTTP API at
/// `OLLAMA_HOST/api/tags`. Compatibility keeps an empty response on discovery
/// failure, while the shared durable catalogue records the normalized error.
pub async fn models(State(state): State<AppState>) -> Json<ApiResponse<OllamaModelsResponse>> {
    let base = resolve_base_url_pub(state.ollama_base_url_override.as_deref());
    match crate::core::model_catalog::refresh_ollama_catalog_at(&state.db, &base).await {
        Ok((_, Ok(tags))) => {
            let listed: Vec<(String, u64, String)> = tags
                .into_iter()
                .map(|tag| (tag.name, tag.size, tag.modified_at))
                .collect();
            let env_cap = crate::core::child_env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok();
            let machine = crate::agents::ollama_memory::MachineFacts::read();
            let overrides = state
                .config
                .read()
                .await
                .server
                .ollama_context_overrides
                .clone();
            // KT-405 — the DECISION each model's context resolves to must be
            // visible per model, not just discoverable by reading a run's
            // logs after the fact. One /api/show per model — bounded
            // concurrency (Codex review): a dozen pulled models must not turn
            // opening Settings into a burst of a dozen simultaneous local
            // requests, cold-load stalls included.
            const MAX_CONCURRENT_PROBES: usize = 4;
            // KT-943 — the ceiling is per model: its weights are what the
            // listing already says it takes, its cache cost comes from its own
            // metadata, so the one probe also decides the ceiling.
            let probes: Vec<(String, u64)> = listed
                .iter()
                .map(|(name, size, _)| (name.clone(), *size))
                .collect();
            let decided = futures::stream::iter(probes)
                .map(|(name, size)| {
                    let base = base.clone();
                    async move {
                        let profile =
                            crate::agents::runner::ollama_model_profile(&base, &name).await;
                        let ceiling = crate::agents::runner::ollama_machine_ceiling(
                            &machine,
                            &base,
                            &name,
                            profile.as_ref(),
                            Some(size),
                        )
                        .await;
                        (
                            profile.and_then(|profile| profile.context_length()),
                            ceiling,
                        )
                    }
                })
                .buffered(MAX_CONCURRENT_PROBES)
                .collect::<Vec<_>>()
                .await;
            let models = listed
                .into_iter()
                .zip(decided)
                .map(|((name, size, modified), (advertised_context, ceiling))| {
                    let context_override = overrides.get(&name).copied();
                    let cap = crate::agents::runner::resolve_ctx_cap_for_model(
                        env_cap.clone(),
                        &name,
                        &overrides,
                        advertised_context,
                        ceiling,
                    );
                    OllamaModel {
                        name,
                        size: format_size(size),
                        modified,
                        advertised_context,
                        context_ceiling: cap.value,
                        context_override,
                        context_origin: context_origin_label(&cap.origin),
                    }
                })
                .collect();
            Json(ApiResponse::ok(OllamaModelsResponse { models }))
        }
        Ok((_, Err(_))) => Json(ApiResponse::ok(OllamaModelsResponse { models: vec![] })),
        Err(_) => Json(ApiResponse::err(
            "Failed to update the saved Ollama catalogue",
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct OllamaRegistryQuery {
    /// The tags the card suggests, comma-separated: their sizes are wanted.
    suggested: Option<String>,
    /// `true` asks the registry again even where an answer is cached: after an
    /// update, and on an explicit Refresh.
    fresh: Option<bool>,
}

/// `"a:1, b:2,,"` → `["a:1", "b:2"]`, at most as many as one answer may look
/// up. Validity of each name is the registry module's call, not this one's.
fn suggested_from_query(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .take(crate::core::ollama_registry::MAX_LOOKUPS)
        .map(str::to_owned)
        .collect()
}

/// GET /api/ollama/registry?suggested=<tag>,<tag>
///
/// KT-930 — what the official Ollama library says about the installed models
/// (is there an update?) and the suggested tags (how big are they?), without
/// downloading anything. A separate call from `/models` on purpose: that one
/// is local and fast and Settings waits for it, this one asks the internet
/// and must never hold the page up. See `core::ollama_registry` for what is
/// bounded and cached, and for why anything unconfirmed is `unknown`.
pub async fn registry(
    State(state): State<AppState>,
    Query(query): Query<OllamaRegistryQuery>,
) -> Json<ApiResponse<OllamaRegistryResponse>> {
    let base = resolve_base_url_pub(state.ollama_base_url_override.as_deref());
    // No local server to read digests from is no installed model to judge;
    // the suggestions' sizes do not depend on it.
    let installed: Vec<(String, String)> =
        match crate::core::model_catalog::ollama_discovery::discover(&base).await {
            Ok(tags) => tags.into_iter().map(|tag| (tag.name, tag.digest)).collect(),
            Err(_) => Vec::new(),
        };
    let suggested = suggested_from_query(query.suggested.as_deref());
    let library = match state.ollama_registry_override.as_deref() {
        Some(library) => library,
        None => crate::core::ollama_registry::shared(),
    };
    Json(ApiResponse::ok(
        crate::core::ollama_registry::report(
            library,
            &installed,
            &suggested,
            if query.fresh == Some(true) {
                crate::core::ollama_registry::Look::RECHECK
            } else {
                crate::core::ollama_registry::Look::CACHED
            },
        )
        .await,
    ))
}

/// Stable string form of `CtxCapOrigin` for the API — the internal enum's
/// variant names are Rust naming, not a contract; this is.
fn context_origin_label(origin: &crate::agents::runner::CtxCapOrigin) -> String {
    match origin {
        crate::agents::runner::CtxCapOrigin::OperatorOverride => "operator_override",
        crate::agents::runner::CtxCapOrigin::ModelOverride => "model_override",
        crate::agents::runner::CtxCapOrigin::ModelWindow => "model_window",
        crate::agents::runner::CtxCapOrigin::MachineCeiling { .. } => "machine_ceiling",
        crate::agents::runner::CtxCapOrigin::ModelEstimate { .. } => "model_estimate",
        crate::agents::runner::CtxCapOrigin::PortableFallback => "portable_fallback",
    }
    .to_string()
}

/// Format bytes into human-readable size (e.g. "4.1 GB").
pub(crate) fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{} B", bytes)
    }
}

/// POST /api/ollama/context-override
///
/// Set or clear (`num_ctx: None`) a persistent per-model context override —
/// KT-405: the persistent, per-model dial, as distinct from
/// `KRONN_OLLAMA_NUM_CTX_CAP` (process-global, gone on restart). Bounds are
/// enforced here — the floor below which Ollama itself misbehaves — but an
/// operator asking for MORE than the model's advertised window or this
/// machine's RAM-derived ceiling is warned, never refused: they may know
/// their machine better than Kronn's coarse RAM tiers do.
/// Pure decision behind the setter's warnings, isolated so it is testable
/// without a live Ollama or a config write. Never a refusal — see the
/// endpoint's doc comment for why. Both facts are independent and both are
/// reported: a value can be over the model's own window AND over what this
/// machine's RAM would otherwise allow, and collapsing that into "one
/// warning wins" would silently drop whichever fact lost.
fn override_warnings(
    model: &str,
    value: u64,
    advertised: Option<u64>,
    ram_ceiling: u64,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(limit) = advertised {
        if value > limit {
            warnings.push(format!(
                "{value} exceeds {model}'s advertised {limit}-token context — Ollama will \
                 likely reject or silently clamp requests at this size."
            ));
        }
    }
    if value > ram_ceiling {
        warnings.push(format!(
            "{value} exceeds the {ram_ceiling}-token ceiling this machine's memory would \
             otherwise allow — only proceed if you know this machine has the RAM a real \
             run at this size needs."
        ));
    }
    warnings
}

/// Sane upper bound against a fat-fingered value (an extra zero on a paste)
/// rather than any real model's capability — the largest local contexts in
/// practice are in the low hundreds of thousands. An operator who genuinely
/// needs more still has `KRONN_OLLAMA_NUM_CTX_CAP`, unbounded, as the
/// break-glass this ceiling deliberately does not cover.
/// Bound on the model tag itself: it is a map key persisted to disk and
/// echoed back verbatim, never executed — this exists only against an
/// accidental paste of something enormous, not a security boundary.
const MAX_MODEL_NAME_LEN: usize = 256;
const PULL_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PULL_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PULL_LINE_BYTES: usize = 1024 * 1024;
const MAX_PULL_ERROR_BYTES: usize = 64 * 1024;

/// A deliberately narrow representation of Ollama's NDJSON pull records.
/// Keeping the upstream shape private means the browser only has to handle
/// the progress contract documented by `OllamaPullProgress`.
#[derive(Debug, serde::Deserialize)]
struct OllamaPullRecord {
    status: Option<String>,
    digest: Option<String>,
    completed: Option<u64>,
    total: Option<u64>,
    error: Option<String>,
}

fn normalize_pull_record(record: OllamaPullRecord) -> Result<OllamaPullProgress, String> {
    if let Some(error) = record.error.filter(|error| !error.trim().is_empty()) {
        return Err(classify_pull_error(&error));
    }
    let status = record
        .status
        .filter(|status| !status.trim().is_empty())
        .ok_or_else(|| {
            "Ollama returned an invalid pull progress record. Try again or update Ollama."
                .to_string()
        })?;
    Ok(OllamaPullProgress {
        status,
        digest: record.digest,
        completed: record.completed,
        total: record.total,
    })
}

fn classify_pull_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if lower.contains("no space left") || lower.contains("disk full") {
        "Ollama could not finish because the disk is full. Free disk space, then try again.".into()
    } else if lower.contains("not found") || lower.contains("manifest unknown") {
        "Ollama could not find this model. Check its name and tag, then try again.".into()
    } else if lower.contains("connection") || lower.contains("network") || lower.contains("timeout")
    {
        "Ollama lost network access while downloading. Check your connection, then try again."
            .into()
    } else {
        format!("Ollama could not download the model: {error}")
    }
}

fn pull_sse_error(message: impl Into<String>) -> Sse<OllamaSseStream> {
    let message = message.into();
    let stream: OllamaSseStream = Box::pin(futures::stream::once(async move {
        Ok(Event::default()
            .event("error")
            .data(serde_json::json!({ "message": message }).to_string()))
    }));
    Sse::new(crate::core::sse_limits::bounded(stream))
}

async fn bounded_response_text(response: reqwest::Response) -> String {
    let mut body = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = body.next().await {
        let Ok(chunk) = chunk else { break };
        let remaining = MAX_PULL_ERROR_BYTES.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if bytes.len() == MAX_PULL_ERROR_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// POST /api/ollama/pull
///
/// Proxies Ollama's NDJSON `POST /api/pull` response as SSE. The request body
/// owns the upstream response, so when the browser aborts the SSE request the
/// response body is dropped and the in-flight Ollama request is cancelled too.
///
/// This is also how an installed model is UPDATED (KT-930): asking Ollama to
/// pull a tag it already holds fetches whatever that tag points to now, and the
/// same progress, cancellation and error contract applies. There is no separate
/// "update" endpoint to drift from the download one.
pub async fn pull(
    State(state): State<AppState>,
    Json(request): Json<PullOllamaModelRequest>,
) -> Sse<OllamaSseStream> {
    let base = resolve_base_url_pub(state.ollama_base_url_override.as_deref());
    pull_from(&base, request).await
}

async fn pull_from(base: &str, request: PullOllamaModelRequest) -> Sse<OllamaSseStream> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        return pull_sse_error("Choose a model before starting the download.");
    }
    if model.chars().count() > MAX_MODEL_NAME_LEN {
        return pull_sse_error(format!(
            "The model name is longer than {MAX_MODEL_NAME_LEN} characters. Check the model name and try again."
        ));
    }

    let client = match reqwest::Client::builder()
        .connect_timeout(PULL_CONNECT_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return pull_sse_error(format!("Could not start the Ollama download: {error}"))
        }
    };
    let upstream = client
        .post(format!("{}/api/pull", base.trim_end_matches('/')))
        .json(&serde_json::json!({ "model": model, "stream": true }))
        .send();
    let response = match tokio::time::timeout(PULL_HEADER_TIMEOUT, upstream).await {
        Err(_) => return pull_sse_error("Ollama did not start the download within 30 seconds. Check that it is responsive, then try again."),
        Ok(result) => match result {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let detail = tokio::time::timeout(PULL_HEADER_TIMEOUT, bounded_response_text(response))
                .await
                .unwrap_or_else(|_| "timeout reading Ollama's error response".to_string());
            return pull_sse_error(classify_pull_error(&detail));
        }
        Err(error) => {
            return pull_sse_error(format!(
                "Could not reach Ollama to start the download. Check that it is running and reachable, then try again. ({error})"
            ));
        }
        },
    };

    Sse::new(crate::core::sse_limits::bounded(pull_events(
        response.bytes_stream(),
    )))
}

/// Turns Ollama's NDJSON pull records into the SSE events the browser reads:
/// `progress` for each record, then exactly one terminal `success` or `error`.
/// Generic over the byte source so the contract (a download and an update of an
/// installed tag share it) is testable without a socket.
fn pull_events<S, E>(mut lines: S) -> OllamaSseStream
where
    S: Stream<Item = Result<axum::body::Bytes, E>> + Send + Unpin + 'static,
    E: std::fmt::Display + Send + 'static,
{
    Box::pin(async_stream::stream! {
        let mut pending = String::new();
        while let Some(chunk) = lines.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    yield Ok(Event::default().event("error").data(
                        serde_json::json!({ "message": format!("The Ollama download stream stopped unexpectedly. Check your network, then try again. ({error})") }).to_string(),
                    ));
                    return;
                }
            };
            if pending.len().saturating_add(chunk.len()) > MAX_PULL_LINE_BYTES {
                yield Ok(Event::default().event("error").data(
                    serde_json::json!({ "message": "Ollama sent an oversized download update. Update Ollama, then try again." }).to_string(),
                ));
                return;
            }
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = pending.find('\n') {
                let line = pending[..newline].trim().to_string();
                pending.drain(..=newline);
                if line.is_empty() { continue; }
                match serde_json::from_str::<OllamaPullRecord>(&line)
                    .map_err(|_| "Ollama returned an invalid pull progress record. Try again or update Ollama.".to_string())
                    .and_then(normalize_pull_record)
                {
                    Ok(progress) => {
                        let finished = progress.status == "success";
                        yield Ok(Event::default().event(if finished { "success" } else { "progress" }).data(
                            serde_json::to_string(&progress).unwrap_or_else(|_| "{}".to_string()),
                        ));
                        if finished { return; }
                    }
                    Err(error) => {
                        yield Ok(Event::default().event("error").data(
                            serde_json::json!({ "message": error }).to_string(),
                        ));
                        return;
                    }
                }
            }
        }
        if !pending.trim().is_empty() {
            match serde_json::from_str::<OllamaPullRecord>(pending.trim())
                .map_err(|_| "Ollama returned an invalid pull progress record. Try again or update Ollama.".to_string())
                .and_then(normalize_pull_record)
            {
                Ok(progress) if progress.status == "success" => yield Ok(Event::default().event("success").data(serde_json::to_string(&progress).unwrap_or_else(|_| "{}".to_string()))),
                Ok(_) => yield Ok(Event::default().event("error").data(serde_json::json!({ "message": "Ollama ended the download without confirming success. You can safely try again." }).to_string())),
                Err(error) => yield Ok(Event::default().event("error").data(serde_json::json!({ "message": error }).to_string())),
            }
        } else {
            yield Ok(Event::default().event("error").data(serde_json::json!({ "message": "Ollama ended the download without confirming success. You can safely try again." }).to_string()));
        }
    })
}

pub async fn set_context_override(
    State(state): State<AppState>,
    Json(request): Json<SetOllamaContextOverrideRequest>,
) -> Json<ApiResponse<SetOllamaContextOverrideResponse>> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        return Json(ApiResponse::err("`model` must not be empty.".to_string()));
    }
    if model.chars().count() > MAX_MODEL_NAME_LEN {
        return Json(ApiResponse::err(format!(
            "`model` is longer than {MAX_MODEL_NAME_LEN} characters; that is not a real \
             Ollama tag."
        )));
    }
    if let Some(value) = request.num_ctx {
        if value < crate::agents::runner::OLLAMA_NUM_CTX_FLOOR {
            return Json(ApiResponse::err(format!(
                "num_ctx must be at least {} tokens; {value} is below what Ollama can \
                 usefully run.",
                crate::agents::runner::OLLAMA_NUM_CTX_FLOOR
            )));
        }
        if value > crate::agents::runner::OLLAMA_NUM_CTX_OVERRIDE_MAX {
            return Json(ApiResponse::err(format!(
                "num_ctx must be at most {} tokens; {value} is \
                 almost certainly a mistake. KRONN_OLLAMA_NUM_CTX_CAP has no such ceiling \
                 if you genuinely need more.",
                crate::agents::runner::OLLAMA_NUM_CTX_OVERRIDE_MAX
            )));
        }
    }

    let warnings = match request.num_ctx {
        Some(value) => {
            let base = ollama_base_url();
            let profile = crate::agents::runner::ollama_model_profile(&base, &model).await;
            let advertised = profile
                .as_ref()
                .and_then(|profile| profile.context_length());
            let ceiling = crate::agents::runner::ollama_machine_ceiling(
                &crate::agents::ollama_memory::MachineFacts::read(),
                &base,
                &model,
                profile.as_ref(),
                crate::agents::runner::ollama_model_size_bytes(&base, &model).await,
            )
            .await;
            override_warnings(&model, value, advertised, ceiling.tokens)
        }
        None => Vec::new(),
    };

    let mut config = state.config.write().await;
    // KT-405 review — a failed save must not leave the in-memory config
    // ahead of what is actually on disk: the process would answer future
    // reads with a value it never durably committed. Mutate a clone, only
    // adopt it once `save` has actually succeeded.
    let mut next = config.clone();
    let previous_value = match request.num_ctx {
        Some(value) => next
            .server
            .ollama_context_overrides
            .insert(model.clone(), value),
        None => next.server.ollama_context_overrides.remove(&model),
    };
    let _ = previous_value; // Not needed by the caller; named for the reader.
    if let Err(error) = crate::core::config::save(&next).await {
        return Json(ApiResponse::err(format!(
            "Failed to save — the previous value is still in effect: {error}"
        )));
    }
    *config = next;
    Json(ApiResponse::ok(SetOllamaContextOverrideResponse {
        model,
        num_ctx: request.num_ctx,
        warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_runner_endpoint_is_used_verbatim() {
        let mock_endpoint = "http://127.0.0.1:43123";
        assert_eq!(resolve_base_url_pub(Some(mock_endpoint)), mock_endpoint);
    }

    /// KT-930 — `-mlx` models are offered only to a Mac on Apple Silicon whose
    /// Ollama is at or above the version Kronn has measured MLX on. Every
    /// other combination, an unreadable version included, offers nothing.
    #[test]
    fn mlx_is_offered_only_on_apple_silicon_with_a_recent_enough_ollama() {
        assert!(mlx_capable(true, Some("0.34.2")));
        assert!(mlx_capable(true, Some("v0.34.0")));
        assert!(mlx_capable(true, Some("0.40.1")));
        assert!(mlx_capable(true, Some("1.0.0")));
        // Apple Silicon, but a server older than the measured floor.
        assert!(!mlx_capable(true, Some("0.33.9")));
        assert!(!mlx_capable(true, Some("0.19.0")));
        // Recent server on any other host (Intel Mac, Linux, WSL, Windows).
        assert!(!mlx_capable(false, Some("0.34.2")));
        assert!(!mlx_capable(false, Some("1.0.0")));
        // The version is the one fact Kronn does not guess.
        assert!(!mlx_capable(true, None));
        assert!(!mlx_capable(true, Some("nightly")));
        assert!(!mlx_capable(true, Some("")));
        assert!(!mlx_capable(false, None));
    }

    #[test]
    fn the_version_body_is_read_as_written_or_not_at_all() {
        use serde_json::json;
        assert_eq!(
            version_from_body(&json!({ "version": "0.34.2" })).as_deref(),
            Some("0.34.2")
        );
        assert_eq!(
            version_from_body(&json!({ "version": " 0.34.2\n" })).as_deref(),
            Some("0.34.2")
        );
        assert_eq!(version_from_body(&json!({ "version": "  " })), None);
        assert_eq!(version_from_body(&json!({ "version": 34 })), None);
        assert_eq!(version_from_body(&json!({})), None);
        assert_eq!(version_from_body(&json!(null)), None);
    }

    #[test]
    fn the_suggested_tags_of_a_query_are_split_trimmed_and_bounded() {
        assert!(suggested_from_query(None).is_empty());
        assert!(suggested_from_query(Some("")).is_empty());
        assert!(suggested_from_query(Some(" , ,")).is_empty());
        assert_eq!(
            suggested_from_query(Some("qwen3:8b, gemma4:12b-mlx,,qwen3.5:4b ")),
            vec!["qwen3:8b", "gemma4:12b-mlx", "qwen3.5:4b"]
        );
        let many = (0..500)
            .map(|i| format!("m{i}:1"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            suggested_from_query(Some(&many)).len(),
            crate::core::ollama_registry::MAX_LOOKUPS
        );
    }

    #[test]
    fn an_online_answer_carries_the_server_version_and_the_mlx_verdict() {
        let mac = online_health(
            "http://localhost:11434".into(),
            3,
            Some("0.34.2".into()),
            true,
        );
        assert_eq!(mac.status, "online");
        assert_eq!(mac.version.as_deref(), Some("0.34.2"));
        assert!(mac.mlx_capable);
        assert!(mac.hint.is_none());

        let linux = online_health(
            "http://localhost:11434".into(),
            3,
            Some("0.34.2".into()),
            false,
        );
        assert_eq!(linux.version.as_deref(), Some("0.34.2"));
        assert!(!linux.mlx_capable, "the same server on a non-Mac host");

        let old_mac = online_health(
            "http://localhost:11434".into(),
            0,
            Some("0.32.14".into()),
            true,
        );
        assert!(!old_mac.mlx_capable);
        assert!(old_mac.hint.is_some(), "an empty install keeps its hint");

        let silent = online_health("http://localhost:11434".into(), 1, None, true);
        assert!(silent.version.is_none());
        assert!(!silent.mlx_capable);
    }

    /// A mock Ollama bound to an ephemeral port: `/api/version` answers what the
    /// test gives it, `/api/pull` records the body it received and streams the
    /// NDJSON records it is given.
    async fn mock_ollama(
        version_body: &'static str,
        pull_lines: &'static str,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::routing::{get, post};
        let pulls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = pulls.clone();
        let app = axum::Router::new()
            .route(
                "/api/version",
                get(move || async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        version_body,
                    )
                }),
            )
            .route(
                "/api/pull",
                post(move |Json(body): Json<serde_json::Value>| {
                    let recorded = recorded.clone();
                    async move {
                        recorded.lock().unwrap().push(body);
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
                            pull_lines,
                        )
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{address}"), pulls, server)
    }

    #[tokio::test]
    async fn the_version_probe_reads_the_server_and_never_guesses() {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let (base, _, server) = mock_ollama(r#"{"version":"0.34.2"}"#, "").await;
        assert_eq!(
            fetch_server_version(&client, &base).await.as_deref(),
            Some("0.34.2")
        );
        server.abort();

        let (base, _, server) = mock_ollama(r#"{"version":"  "}"#, "").await;
        assert_eq!(fetch_server_version(&client, &base).await, None);
        server.abort();

        let (base, _, server) = mock_ollama("not json", "").await;
        assert_eq!(fetch_server_version(&client, &base).await, None);
        server.abort();

        // Nothing listening: the probe fails closed instead of inventing one.
        let (base, _, server) = mock_ollama("{}", "").await;
        server.abort();
        let _ = server.await;
        assert_eq!(fetch_server_version(&client, &base).await, None);
    }

    async fn sse_body(sse: Sse<OllamaSseStream>) -> String {
        use axum::response::IntoResponse;
        use http_body_util::BodyExt;
        let bytes = sse
            .into_response()
            .into_body()
            .collect()
            .await
            .expect("the SSE body ends")
            .to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// KT-930 — updating an installed model is the download flow pointed at a
    /// tag the server already holds: the same upstream call, with the exact tag
    /// (trimmed), streamed back as the same progress and terminal events. Needs
    /// a local socket for the mock server, which a sandbox may refuse.
    #[tokio::test]
    async fn updating_an_installed_tag_streams_the_same_pull_as_a_download() {
        let (base, pulls, server) = mock_ollama(
            "{}",
            "{\"status\":\"pulling manifest\"}\n\
             {\"status\":\"downloading\",\"digest\":\"sha256:abc\",\"completed\":1048576,\"total\":4194304}\n\
             {\"status\":\"success\"}\n",
        )
        .await;

        let body = sse_body(
            pull_from(
                &base,
                PullOllamaModelRequest {
                    model: "  qwen3.8:27b-mlx ".into(),
                },
            )
            .await,
        )
        .await;

        let sent = pulls.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "one upstream pull");
        assert_eq!(
            sent[0]["model"], "qwen3.8:27b-mlx",
            "the exact tag, trimmed"
        );
        assert_eq!(sent[0]["stream"], true);
        assert!(body.contains("event: progress"), "{body}");
        assert!(body.contains("\"completed\":1048576"), "{body}");
        assert!(body.contains("\"total\":4194304"), "{body}");
        assert!(body.contains("event: success"), "{body}");
        assert!(!body.contains("event: error"), "{body}");
        server.abort();
    }

    /// The browser-facing events for a stream of NDJSON chunks, with no socket:
    /// what `pull_events` does to whatever Ollama sends back, a download of a
    /// new tag and an update of an installed one alike.
    async fn pull_events_for(chunks: &[&'static str]) -> String {
        let source = futures::stream::iter(
            chunks
                .iter()
                .map(|chunk| Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(*chunk)))
                .collect::<Vec<_>>(),
        );
        sse_body(Sse::new(pull_events(source))).await
    }

    #[tokio::test]
    async fn an_update_stream_reports_progress_then_exactly_one_success() {
        let body = pull_events_for(&[
            "{\"status\":\"pulling manifest\"}\n",
            "{\"status\":\"downloading\",\"digest\":\"sha256:abc\",\"completed\":1048576,\"total\":4194304}\n",
            "{\"status\":\"success\"}\n",
        ])
        .await;
        assert_eq!(body.matches("event: progress").count(), 2, "{body}");
        assert!(body.contains("\"completed\":1048576"), "{body}");
        assert!(body.contains("\"total\":4194304"), "{body}");
        assert_eq!(body.matches("event: success").count(), 1, "{body}");
        assert!(!body.contains("event: error"), "{body}");
    }

    /// A stream that never carries a byte counter (nothing to fetch, or a
    /// server that does not report one) is still a finished pull.
    #[tokio::test]
    async fn a_stream_without_byte_counters_still_ends_in_success() {
        let body =
            pull_events_for(&["{\"status\":\"pulling manifest\"}\n{\"status\":\"success\"}\n"])
                .await;
        assert!(body.contains("event: success"), "{body}");
        assert!(!body.contains("event: error"), "{body}");
    }

    #[tokio::test]
    async fn records_split_across_chunks_are_reassembled() {
        let body = pull_events_for(&[
            "{\"status\":\"downloading\",\"completed\":10,",
            "\"total\":20}\n{\"status\":\"succ",
            "ess\"}\n",
        ])
        .await;
        assert!(body.contains("\"completed\":10"), "{body}");
        assert!(body.contains("event: success"), "{body}");
        assert!(!body.contains("event: error"), "{body}");
    }

    #[tokio::test]
    async fn a_refused_update_surfaces_an_error_and_never_a_success() {
        let body = pull_events_for(&[
            "{\"status\":\"pulling manifest\"}\n",
            "{\"error\":\"pull model manifest: file does not exist\"}\n",
        ])
        .await;
        assert!(body.contains("event: error"), "{body}");
        assert!(body.contains("pull model manifest"), "{body}");
        assert!(!body.contains("event: success"), "{body}");
    }

    #[tokio::test]
    async fn a_stream_that_stops_before_success_is_an_error_not_a_silent_success() {
        let body =
            pull_events_for(&["{\"status\":\"downloading\",\"completed\":5,\"total\":9}\n"]).await;
        assert!(body.contains("event: error"), "{body}");
        assert!(body.contains("without confirming success"), "{body}");
        assert!(!body.contains("event: success"), "{body}");
    }

    /// KT-405 — an operator's request is never refused for being LARGER than
    /// what Kronn's own figures suggest; it is warned. The floor rejection
    /// (values that would break Ollama outright) lives in the handler and is
    /// exercised through the full request in `set_context_override`'s own
    /// tests, not here — this is only the warning DECISION.
    #[test]
    fn override_warnings_name_every_figure_exceeded() {
        assert!(
            override_warnings("qwen3.8:27b-mlx", 50_000, Some(262_144), 65_536).is_empty(),
            "under both the model's window and this machine's RAM ceiling"
        );
        // Advertised window BELOW the RAM ceiling here on purpose: a value
        // between the two exceeds only the model's own window, not the
        // machine's — the two facts must stay independently reportable.
        let over_model = override_warnings("qwen3.8:27b-mlx", 50_000, Some(40_000), 65_536);
        assert_eq!(over_model.len(), 1, "{over_model:?}");
        assert!(over_model[0].contains("40000"), "{over_model:?}");
        let over_ram = override_warnings("qwen3.8:27b-mlx", 100_000, None, 65_536);
        assert_eq!(over_ram.len(), 1, "{over_ram:?}");
        assert!(over_ram[0].contains("65536"), "{over_ram:?}");
        // Under the model's advertised window but still above the RAM
        // ceiling: the operator overriding past Kronn's RAM heuristic is
        // exactly the case that owes a warning, not a free pass.
        let over_ram_under_model =
            override_warnings("qwen3.8:27b-mlx", 100_000, Some(262_144), 65_536);
        assert_eq!(over_ram_under_model.len(), 1, "{over_ram_under_model:?}");
        assert!(
            over_ram_under_model[0].contains("65536"),
            "{over_ram_under_model:?}"
        );

        // KT-405 review — both facts are independent and both must survive: a
        // value over BOTH the model's window and this machine's RAM ceiling
        // must report two warnings, not just the first one hit.
        let over_both = override_warnings("qwen3.8:27b-mlx", 500_000, Some(262_144), 65_536);
        assert_eq!(over_both.len(), 2, "{over_both:?}");
        assert!(
            over_both.iter().any(|w| w.contains("262144")),
            "{over_both:?}"
        );
        assert!(
            over_both.iter().any(|w| w.contains("65536")),
            "{over_both:?}"
        );
    }

    fn test_state() -> crate::AppState {
        use std::sync::Arc;
        use tokio::sync::RwLock;
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let config = Arc::new(RwLock::new(crate::core::config::default_config()));
        crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS)
    }

    async fn set(
        state: &crate::AppState,
        model: &str,
        num_ctx: Option<u64>,
    ) -> crate::models::ApiResponse<SetOllamaContextOverrideResponse> {
        set_context_override(
            axum::extract::State(state.clone()),
            axum::Json(SetOllamaContextOverrideRequest {
                model: model.to_string(),
                num_ctx,
            }),
        )
        .await
        .0
    }

    /// KT-405 — a value below the floor Ollama can usefully run must be
    /// refused outright, not merely warned about: unlike "too large", there
    /// is no scenario where the operator's machine makes it correct.
    #[tokio::test]
    async fn a_value_below_the_floor_is_refused_not_warned() {
        let state = test_state();
        let response = set(&state, "qwen3:8b", Some(512)).await;
        assert!(
            !response.success,
            "512 is below the floor and must be refused: {response:?}"
        );
    }

    /// KT-405 review — a value that is not merely large but absurd (an extra
    /// zero on a paste) must be refused, with the break-glass named so the
    /// rare operator who genuinely needs more is not left stuck.
    #[tokio::test]
    async fn a_value_far_past_any_real_model_is_refused_with_the_escape_hatch_named() {
        let state = test_state();
        let response = set(&state, "qwen3:8b", Some(50_000_000)).await;
        assert!(!response.success, "{response:?}");
        let message = response.error.unwrap_or_default();
        assert!(
            message.contains("KRONN_OLLAMA_NUM_CTX_CAP"),
            "the refusal must name the way out for a genuine outlier: {message}"
        );
    }

    /// KT-405 — the label crossing into the API is a stable string, not the
    /// Rust variant name. Pinned so a rename inside `runner.rs` cannot change
    /// what a frontend receives without this test noticing.
    #[test]
    fn context_origin_label_is_stable_across_every_variant() {
        use crate::agents::runner::CtxCapOrigin;
        assert_eq!(
            context_origin_label(&CtxCapOrigin::OperatorOverride),
            "operator_override"
        );
        assert_eq!(
            context_origin_label(&CtxCapOrigin::ModelOverride),
            "model_override"
        );
        assert_eq!(
            context_origin_label(&CtxCapOrigin::ModelWindow),
            "model_window"
        );
        assert_eq!(
            context_origin_label(&CtxCapOrigin::MachineCeiling {
                model_limit: 262_144
            }),
            "machine_ceiling"
        );
        assert_eq!(
            context_origin_label(&CtxCapOrigin::ModelEstimate {
                model_limit: 262_144
            }),
            "model_estimate"
        );
        assert_eq!(
            context_origin_label(&CtxCapOrigin::PortableFallback),
            "portable_fallback"
        );
    }

    #[test]
    fn pull_progress_preserves_bytes_and_unknown_totals() {
        let progress = normalize_pull_record(OllamaPullRecord {
            status: Some("downloading".into()),
            digest: Some("sha256:abc".into()),
            completed: Some(1_048_576),
            total: None,
            error: None,
        })
        .expect("valid partial progress");
        assert_eq!(progress.completed, Some(1_048_576));
        assert_eq!(progress.total, None);
        assert_eq!(progress.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn pull_success_is_a_normalized_terminal_record() {
        let progress = normalize_pull_record(OllamaPullRecord {
            status: Some("success".into()),
            digest: None,
            completed: None,
            total: None,
            error: None,
        })
        .expect("success is a valid record");
        assert_eq!(progress.status, "success");
    }

    #[test]
    fn pull_errors_are_actionable_and_invalid_records_are_refused() {
        let disk_error = normalize_pull_record(OllamaPullRecord {
            status: None,
            digest: None,
            completed: None,
            total: None,
            error: Some("write: no space left on device".into()),
        })
        .expect_err("disk-full upstream error must stop the pull");
        assert!(disk_error.contains("disk is full"), "{disk_error}");

        let invalid = normalize_pull_record(OllamaPullRecord {
            status: None,
            digest: None,
            completed: None,
            total: None,
            error: None,
        })
        .expect_err("a record without status or error is invalid");
        assert!(invalid.contains("invalid pull progress"), "{invalid}");
    }

    #[test]
    fn pull_error_classifier_distinguishes_network_and_unknown_models() {
        assert!(classify_pull_error("manifest unknown").contains("could not find this model"));
        assert!(classify_pull_error("network timeout").contains("network access"));
    }

    #[test]
    fn pull_bounds_allow_long_downloads_but_reject_unbounded_records() {
        assert!(PULL_CONNECT_TIMEOUT < PULL_HEADER_TIMEOUT);
        const {
            assert!(MAX_PULL_LINE_BYTES >= 1024 * 1024);
            assert!(MAX_PULL_ERROR_BYTES < MAX_PULL_LINE_BYTES);
        }
    }

    #[tokio::test]
    async fn dropping_a_pull_sse_closes_its_upstream_request() {
        use axum::response::IntoResponse;
        use http_body_util::BodyExt;
        use tokio::sync::oneshot;

        let (closed_tx, closed_rx) = oneshot::channel();
        struct DropNotify(Option<oneshot::Sender<()>>);
        impl Drop for DropNotify {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let stream: OllamaSseStream = Box::pin(async_stream::stream! {
            let _upstream_response = DropNotify(Some(closed_tx));
            futures::future::pending::<()>().await;
            yield Ok(Event::default());
        });
        let mut body = Sse::new(stream).into_response().into_body();
        let reader = tokio::spawn(async move { body.frame().await });
        tokio::task::yield_now().await;
        reader.abort();
        let _ = reader.await;

        tokio::time::timeout(std::time::Duration::from_secs(1), closed_rx)
            .await
            .expect("cancelling the SSE response drops its upstream owner")
            .expect("upstream owner reports cancellation");
    }
}
