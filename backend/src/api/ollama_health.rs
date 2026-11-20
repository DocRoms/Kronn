//! Ollama health probe and base-URL resolution, kept below `api::ollama` so
//! the orchestration layer can probe Ollama without depending on its routes.

use crate::models::*;
use crate::AppState;
use axum::{extract::State, Json};
use std::time::Duration;

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
pub fn ollama_base_url() -> String {
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
pub(super) fn detect_context() -> &'static str {
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
pub async fn fetch_server_version(client: &reqwest::Client, base: &str) -> Option<String> {
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
pub fn version_from_body(body: &serde_json::Value) -> Option<String> {
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
pub fn mlx_capable(apple_silicon: bool, version: Option<&str>) -> bool {
    apple_silicon
        && crate::agents::runner::mlx_prefix_cache_reused(
            version.and_then(crate::agents::runner::parse_ollama_version),
        )
}

/// The health answer for a reachable server, from facts already gathered.
pub fn online_health(
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
