use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A named OpenAI-compatible API connection. This model persists only the
/// credential's slug; the value is an `ApiKey` kept in the encrypted
/// `stored_credentials` table, outside the residual cases listed in
/// TD-20260901-plaintext-connection-credentials.
#[derive(Clone, PartialEq, Serialize, Deserialize, TS)]
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
    /// The model Decision steps call on this connection (KT-1366). A
    /// capability like the media slots, not a quality tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_model: Option<String>,
    /// Operator switch for the Kronn tool executor on runs on this connection
    /// (KT-1600). Tools are on by default: `None` and `Some(true)` attach
    /// them, `Some(false)` withholds them, for a model that cannot take tool
    /// calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_enabled: Option<bool>,
}

// Every field as written, except the userinfo of the endpoints (KT-2055): an
// endpoint may carry `user:pass@`.
impl std::fmt::Debug for ExternalApiConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            id,
            display_name,
            mention_alias,
            endpoint,
            credential_slug,
            origin_preset,
            economy_model,
            default_model,
            reasoning_model,
            created_at,
            updated_at,
            image_model,
            video_model,
            media_endpoint,
            decision_model,
            tools_enabled,
        } = self;
        let masked = |url: &Option<String>| {
            url.as_deref()
                .map(|url| crate::core::redact::mask_url_userinfo(url).into_owned())
        };
        f.debug_struct("ExternalApiConnection")
            .field("id", id)
            .field("display_name", display_name)
            .field("mention_alias", mention_alias)
            .field("endpoint", &masked(endpoint))
            .field("credential_slug", credential_slug)
            .field("origin_preset", origin_preset)
            .field("economy_model", economy_model)
            .field("default_model", default_model)
            .field("reasoning_model", reasoning_model)
            .field("created_at", created_at)
            .field("updated_at", updated_at)
            .field("image_model", image_model)
            .field("video_model", video_model)
            .field("media_endpoint", &masked(media_endpoint))
            .field("decision_model", decision_model)
            .field("tools_enabled", tools_enabled)
            .finish()
    }
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

impl ExternalApiConnectionPreset {
    /// Hosted NVIDIA and OpenRouter endpoints refuse an anonymous call; a
    /// LiteLLM proxy or another self-hosted server may accept one.
    pub fn requires_credential(self) -> bool {
        matches!(self, Self::Nvidia | Self::OpenRouter)
    }
}
