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
    /// Media generation slots. Modalities, NOT quality tiers: making them
    /// ModelTier variants would let a text step select "tier Image".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_model: Option<String>,
    /// Override for providers serving media from another host than their chat
    /// endpoint. Empty means "derive from `endpoint`".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_endpoint: Option<String>,
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
