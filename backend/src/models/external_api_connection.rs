use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A named OpenAI-compatible API connection. The credential itself stays in
/// Kronn's encrypted credential store; this model persists only its slug.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExternalApiConnection {
    pub id: String,
    pub display_name: String,
    pub mention_alias: String,
    pub endpoint: Option<String>,
    pub credential_slug: String,
    pub origin_preset: ExternalApiConnectionPreset,
    pub economy_model: Option<String>,
    pub default_model: Option<String>,
    pub reasoning_model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExternalApiConnectionPreset {
    LiteLlm,
    Nvidia,
    OpenRouter,
    Other,
}
