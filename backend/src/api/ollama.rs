//! Ollama local LLM endpoints (v0.4.0 — Phase 1).
//!
//! Health check and model listing via Ollama's HTTP API. The actual
//! agent execution goes through the standard `agent_command()` path
//! in `runner.rs` which spawns `ollama run <model>`.
//!
//! Ollama runs on the HOST machine (not in the Docker container).
//! In Docker, we reach it via `host.docker.internal:11434`.

use crate::models::*;
use crate::AppState;
use axum::{
    extract::State,
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
    if let Ok(host) = std::env::var("OLLAMA_HOST") {
        if !host.is_empty() && host != "0.0.0.0" {
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
    match std::env::var("KRONN_HOST_OS").as_deref() {
        Ok("WSL") => "docker_wsl",
        Ok("macOS") => "docker_macos",
        _ => "docker_linux",
    }
}

/// GET /api/ollama/health
///
/// Probe Ollama availability with contextual error messages.
/// The `hint` field provides a user-friendly explanation adapted to the
/// detected environment (native, Docker on WSL, Docker on macOS, etc.).
pub async fn health(State(_state): State<AppState>) -> Json<ApiResponse<OllamaHealthResponse>> {
    let base = ollama_base_url();
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

            let hint = if models_count == 0 {
                Some("Ollama est en ligne mais aucun modèle n'est installé. Exécutez : ollama pull qwen3:8b".into())
            } else {
                None
            };

            Json(ApiResponse::ok(OllamaHealthResponse {
                status: "online".into(),
                version: None,
                endpoint: base,
                models_count,
                hint,
            }))
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
            }))
        }
    }
}

/// GET /api/ollama/models
///
/// List locally installed Ollama models. Uses the HTTP API at
/// `OLLAMA_HOST/api/tags`. Returns an empty list if Ollama is unreachable.
pub async fn models(State(state): State<AppState>) -> Json<ApiResponse<OllamaModelsResponse>> {
    let base = ollama_base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client.get(format!("{}/api/tags", base)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            // One tuple per model, in one pass: two separately-filtered vectors
            // zipped back together is how a model missing `name` shifts every
            // later entry's `modified_at` onto the wrong model (Codex review).
            let listed: Vec<(String, u64, String)> = body["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some((
                                m["name"].as_str()?.to_string(),
                                m["size"].as_u64().unwrap_or(0),
                                m["modified_at"].as_str().unwrap_or("").to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let env_cap = std::env::var("KRONN_OLLAMA_NUM_CTX_CAP").ok();
            let ram_ceiling = crate::agents::runner::ram_derived_ceiling(
                crate::agents::runner::total_system_memory_bytes(),
            );
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
            let names_to_probe: Vec<String> =
                listed.iter().map(|(name, _, _)| name.clone()).collect();
            let contexts = futures::stream::iter(names_to_probe)
                .map(|name| {
                    let base = base.clone();
                    async move { crate::agents::runner::ollama_model_ctx_limit(&base, &name).await }
                })
                .buffered(MAX_CONCURRENT_PROBES)
                .collect::<Vec<_>>()
                .await;
            let models = listed
                .into_iter()
                .zip(contexts)
                .map(|((name, size, modified), advertised_context)| {
                    let context_override = overrides.get(&name).copied();
                    let cap = crate::agents::runner::resolve_ctx_cap_for_model(
                        env_cap.clone(),
                        &name,
                        &overrides,
                        advertised_context,
                        ram_ceiling,
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
        _ => Json(ApiResponse::ok(OllamaModelsResponse { models: vec![] })),
    }
}

/// Stable string form of `CtxCapOrigin` for the API — the internal enum's
/// variant names are Rust naming, not a contract; this is.
fn context_origin_label(origin: &crate::agents::runner::CtxCapOrigin) -> String {
    match origin {
        crate::agents::runner::CtxCapOrigin::OperatorOverride => "operator_override",
        crate::agents::runner::CtxCapOrigin::ModelOverride => "model_override",
        crate::agents::runner::CtxCapOrigin::ModelWindow => "model_window",
        crate::agents::runner::CtxCapOrigin::MachineCeiling { .. } => "machine_ceiling",
        crate::agents::runner::CtxCapOrigin::PortableFallback => "portable_fallback",
    }
    .to_string()
}

/// Format bytes into human-readable size (e.g. "4.1 GB").
fn format_size(bytes: u64) -> String {
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
pub async fn pull(Json(request): Json<PullOllamaModelRequest>) -> Sse<OllamaSseStream> {
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
        .post(format!("{}/api/pull", ollama_base_url()))
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

    let stream: OllamaSseStream = Box::pin(async_stream::stream! {
        let mut lines = response.bytes_stream();
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
    });
    Sse::new(crate::core::sse_limits::bounded(stream))
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
            let advertised = crate::agents::runner::ollama_model_ctx_limit(&base, &model).await;
            let ram_ceiling = crate::agents::runner::ram_derived_ceiling(
                crate::agents::runner::total_system_memory_bytes(),
            );
            override_warnings(&model, value, advertised, ram_ceiling)
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
