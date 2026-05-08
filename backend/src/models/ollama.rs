// Ollama (local LLM) integration types — health/probe response, model list,
// and the lightweight model descriptor. Introduced in 0.4.0 when local
// inference became a first-class agent option.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OllamaModel {
    pub name: String,
    pub size: String,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaHealthResponse {
    /// "online", "offline", "not_installed", "unreachable"
    pub status: String,
    pub version: Option<String>,
    pub endpoint: String,
    pub models_count: u32,
    /// User-facing explanation when status != "online". Contextualized
    /// for the detected environment (native, Docker, WSL).
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaModelsResponse {
    pub models: Vec<OllamaModel>,
}
