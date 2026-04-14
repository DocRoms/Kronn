//! Ollama local LLM endpoints (v0.4.0 — Phase 1).
//!
//! Health check and model listing via Ollama's HTTP API. The actual
//! agent execution goes through the standard `agent_command()` path
//! in `runner.rs` which spawns `ollama run <model>`.
//!
//! Ollama runs on the HOST machine (not in the Docker container).
//! In Docker, we reach it via `host.docker.internal:11434`.

use axum::{extract::State, Json};
use crate::models::*;
use crate::AppState;

/// Public accessor for the runner's HTTP execution path.
pub fn ollama_base_url_pub() -> String { ollama_base_url() }

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
pub async fn health(
    State(_state): State<AppState>,
) -> Json<ApiResponse<OllamaHealthResponse>> {
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
            let models_count = body["models"].as_array().map(|a| a.len() as u32).unwrap_or(0);

            let hint = if models_count == 0 {
                Some("Ollama est en ligne mais aucun modèle n'est installé. Exécutez : ollama pull llama3.2".into())
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
pub async fn models(
    State(_state): State<AppState>,
) -> Json<ApiResponse<OllamaModelsResponse>> {
    let base = ollama_base_url();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    match client.get(format!("{}/api/tags", base)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let models = body["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(OllamaModel {
                                name: m["name"].as_str()?.to_string(),
                                size: format_size(m["size"].as_u64().unwrap_or(0)),
                                modified: m["modified_at"].as_str().unwrap_or("").to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Json(ApiResponse::ok(OllamaModelsResponse { models }))
        }
        _ => {
            Json(ApiResponse::ok(OllamaModelsResponse { models: vec![] }))
        }
    }
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
