// LiteLLM (OpenAI-compatible proxy) integration types. Mirrors the Ollama
// pair: LiteLLM is the second server-shaped agent, so "installed" (binary on
// PATH) and "reachable" (proxy answering) are reported separately.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LiteLlmModel {
    /// The id the proxy answers to — what goes in a step's `model_override`.
    pub id: String,
    /// The model actually serving this alias, e.g. `ollama_chat/qwen3:4b`.
    /// `None` when the proxy does not disclose it (`/model/info` is optional
    /// and may be admin-gated on a corporate deployment).
    pub backing_model: Option<String>,
    /// Provider parsed from `backing_model`'s prefix (`ollama`, `azure`, …).
    /// Deliberately NOT `owned_by`: LiteLLM hardcodes that to "openai" for
    /// API compatibility even when a local Ollama model answers, which reads
    /// as false provenance in the UI.
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LiteLlmHealthResponse {
    /// "online", "offline", "not_installed", "unreachable"
    pub status: String,
    pub endpoint: String,
    pub models_count: u32,
    /// User-facing explanation when status != "online".
    pub hint: Option<String>,
    /// Whether an endpoint has ever been saved. Distinguishes "never set up"
    /// from "set up but currently down", which are different cards.
    pub configured: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LiteLlmModelsResponse {
    pub models: Vec<LiteLlmModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LiteLlmModelFailure {
    pub model: String,
    pub status_code: u16,
    pub error_message: String,
    pub first_failed_at: String,
    pub last_failed_at: String,
    pub failure_count: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LiteLlmModelFailuresResponse {
    pub failures: Vec<LiteLlmModelFailure>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct LiteLlmModelRetryRequest {
    pub model: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LiteLlmModelRetryResponse {
    pub healthy: bool,
    pub failure: Option<LiteLlmModelFailure>,
}

/// Connection attempt from the settings card. The key is write-only: it is
/// stored in the encrypted token store and never read back to the frontend.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct LiteLlmTestRequest {
    pub base_url: String,
    /// `None` leaves an already-stored key untouched; `Some("")` clears it.
    #[serde(default)]
    pub api_key: Option<String>,
}

/// Outcome of a connection attempt. `saved` tells the card whether it may
/// move on to model selection — a probe that failed persists nothing.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LiteLlmTestResponse {
    pub ok: bool,
    pub saved: bool,
    pub status: String,
    pub endpoint: String,
    pub models: Vec<LiteLlmModel>,
    pub hint: Option<String>,
}
