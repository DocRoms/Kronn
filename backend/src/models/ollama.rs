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
    /// KT-405 — this model's trained context window, from Ollama's
    /// `/api/show`. `None` when Ollama did not answer (offline, cold load
    /// that outlasted the retry ladder) — never a fallback NUMBER standing in
    /// for a fact Kronn does not have.
    pub advertised_context: Option<u64>,
    /// The CEILING a run against this model would be sized within — not
    /// necessarily the exact `num_ctx` a specific run sends: a short,
    /// tool-free prompt is sized smaller than this ceiling by
    /// `ollama_num_ctx`. This is "how large could it go", computed the same
    /// way a real run computes it (persistent override, else env override,
    /// else advertised context clamped to this machine's RAM ceiling, else
    /// the portable fallback).
    pub context_ceiling: u64,
    /// The persistent value saved for this exact model tag, independent of
    /// which higher-precedence source currently determines the ceiling. For
    /// example, an env override may be active while this remains `Some` so
    /// Settings can still pre-fill, edit or reset the saved value honestly.
    pub context_override: Option<u64>,
    /// Why `context_ceiling` is what it is: "operator_override" |
    /// "model_override" | "model_window" | "machine_ceiling" |
    /// "portable_fallback". A string, not the internal enum — this crosses
    /// into the API surface and a frontend has no reason to know Rust
    /// variant names.
    pub context_origin: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaHealthResponse {
    /// "online", "offline", "not_installed", "unreachable"
    pub status: String,
    /// The server's own version, from `/api/version`. `None` when the server
    /// is not online or did not answer that probe.
    pub version: Option<String>,
    pub endpoint: String,
    pub models_count: u32,
    /// User-facing explanation when status != "online". Contextualized
    /// for the detected environment (native, Docker, WSL).
    pub hint: Option<String>,
    /// KT-930 — this host is a Mac on Apple Silicon AND the running Ollama is
    /// recent enough to run `-mlx` models as Kronn expects. Decided here, from
    /// the host and the server's reported version — never from the browser's
    /// user agent, which says nothing about where Ollama runs. `false` when
    /// either fact is missing or unknown.
    pub mlx_capable: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaModelsResponse {
    pub models: Vec<OllamaModel>,
}

/// KT-930 — how an installed model compares with the official Ollama library's
/// current copy of the same tag.
#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OllamaUpdateStatus {
    /// The library's manifest hashes to the digest installed here.
    UpToDate,
    /// The library's manifest hashes to something else: the tag moved on.
    UpdateAvailable,
    /// No answer to compare against: the registry was unreachable or slow, the
    /// model is not in the official library, or either digest was unreadable.
    /// Never read as up to date.
    Unknown,
}

/// One installed model's freshness.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaModelFreshness {
    pub name: String,
    pub status: OllamaUpdateStatus,
}

/// What a suggested tag weighs, from the library's manifest (the sum of its
/// config and layer sizes), formatted like `OllamaModel::size`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaSuggestionSize {
    pub name: String,
    pub size: String,
}

/// GET /api/ollama/registry — what the official library says about the
/// installed models and the suggested tags, without downloading anything.
/// A tag the library did not answer for is simply absent from `suggestions`
/// and `unknown` in `models`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaRegistryResponse {
    pub models: Vec<OllamaModelFreshness>,
    pub suggestions: Vec<OllamaSuggestionSize>,
}

/// The sole accepted input for a local Ollama pull.  The endpoint never
/// accepts arbitrary upstream URLs: it always uses Kronn's configured Ollama
/// base URL.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PullOllamaModelRequest {
    pub model: String,
}

/// Normalized status sent as an SSE `progress` event while Ollama pulls a
/// model. `completed` and `total` remain optional because several Ollama
/// stages have no byte counter.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct OllamaPullProgress {
    pub status: String,
    pub digest: Option<String>,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetOllamaContextOverrideRequest {
    pub model: String,
    /// `None` clears the override, back to the auto-derived cap.
    pub num_ctx: Option<u64>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SetOllamaContextOverrideResponse {
    pub model: String,
    /// The override actually stored, echoed back so the caller does not have
    /// to re-fetch `/api/ollama/models` to know what stuck.
    pub num_ctx: Option<u64>,
    /// Every independent reason the value is worth a second look — e.g. BOTH
    /// above the model's advertised window AND above this machine's RAM
    /// ceiling, which are two distinct facts, not one. Never blocks the
    /// write: the operator asked for a specific number, and a machine can
    /// genuinely have more RAM free than Kronn's coarse tiers assume.
    pub warnings: Vec<String>,
}
