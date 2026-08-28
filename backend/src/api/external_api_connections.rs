//! Named external API connections — the unified "External API" settings zone.
//!
//! KT-339 — LiteLLM and NVIDIA are not two providers from Kronn's point of
//! view: they are two connections to the SAME OpenAI-compatible contract
//! (`{base}/v1/chat/completions`, `/v1/models`, bearer auth, `OpenAiCodec`).
//! This CRUD surface lets an operator declare any number of such connections
//! from the UI alone — a third compatible service (e.g. Groq) needs no new
//! enum variant, no dedicated card and no new i18n key: it is just an `Other`
//! preset with a user-supplied endpoint.
//!
//! The credential itself never leaves the encrypted token store: a connection
//! references it by its stable `credential_slug`, and the list response only
//! exposes whether a credential is present, never its value.

use crate::core::config;
use crate::db::external_api_connections as store;
use crate::models::*;
use crate::AppState;
use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// A connection as the settings UI sees it: the persisted row plus whether a
/// credential is currently stored for it. The slug is safe to expose (it is an
/// opaque store key, not the secret).
#[derive(Debug, Serialize)]
pub struct ConnectionView {
    #[serde(flatten)]
    pub connection: ExternalApiConnection,
    pub has_credential: bool,
}

/// Create/update payload. `api_key` is write-only and tri-state: `None` keeps
/// the stored credential, `Some("")` clears it, `Some(value)` replaces it.
#[derive(Debug, Deserialize)]
pub struct UpsertConnectionRequest {
    pub display_name: String,
    pub mention_alias: String,
    pub endpoint: Option<String>,
    pub origin_preset: ExternalApiConnectionPreset,
    pub economy_model: Option<String>,
    pub default_model: Option<String>,
    pub reasoning_model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn view(connection: ExternalApiConnection, config: &AppConfig) -> ConnectionView {
    let has_credential = config
        .tokens
        .active_key_for(&connection.credential_slug)
        .is_some_and(|k| !k.trim().is_empty());
    ConnectionView {
        connection,
        has_credential,
    }
}

/// Replace the stored credential for a connection's slug, or clear it when the
/// value is blank. Mirrors `lite_llm::upsert_key` but keyed by the connection's
/// own slug so several connections never share a credential.
fn set_credential(cfg: &mut AppConfig, slug: &str, display_name: &str, value: &str) {
    cfg.tokens.keys.retain(|k| k.provider != slug);
    if value.trim().is_empty() {
        return;
    }
    cfg.tokens.keys.push(ApiKey {
        id: uuid::Uuid::new_v4().to_string(),
        name: format!("{display_name} API key"),
        provider: slug.to_string(),
        value: value.to_string(),
        active: true,
    });
}

/// Build a stable, unique credential slug from the mention alias. A short uuid
/// suffix keeps two connections that reuse a similar alias distinct in the
/// UNIQUE-constrained store.
fn credential_slug_for(alias: &str) -> String {
    let base: String = alias
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "connection" } else { base };
    format!("conn-{base}-{}", &uuid::Uuid::new_v4().to_string()[..8])
}

/// GET /api/external-api/connections
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<ConnectionView>>> {
    let connections = match state.db.with_read_conn(store::list).await {
        Ok(rows) => rows,
        Err(e) => return Json(ApiResponse::err(format!("Failed to list connections: {e}"))),
    };
    let config = state.config.read().await;
    let views = connections
        .into_iter()
        .map(|c| view(c, &config))
        .collect::<Vec<_>>();
    Json(ApiResponse::ok(views))
}

/// POST /api/external-api/connections
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<UpsertConnectionRequest>,
) -> Json<ApiResponse<ConnectionView>> {
    let display_name = req.display_name.trim().to_string();
    let mention_alias = req.mention_alias.trim().to_string();
    if display_name.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Display name required",
        ));
    }
    if mention_alias.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Mention alias required",
        ));
    }

    let now = Utc::now();
    let credential_slug = credential_slug_for(&mention_alias);
    let connection = ExternalApiConnection {
        id: uuid::Uuid::new_v4().to_string(),
        display_name: display_name.clone(),
        mention_alias,
        endpoint: clean(req.endpoint),
        credential_slug: credential_slug.clone(),
        origin_preset: req.origin_preset,
        economy_model: clean(req.economy_model),
        default_model: clean(req.default_model),
        reasoning_model: clean(req.reasoning_model),
        created_at: now,
        updated_at: now,
    };

    // The DB insert enforces the case-insensitive alias uniqueness, so it runs
    // before we ever touch the credential store: a rejected alias must not
    // leave an orphan credential behind.
    let to_insert = connection.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| store::insert(conn, &to_insert))
        .await
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("{e}"),
        ));
    }

    let mut cfg = state.config.write().await;
    if let Some(token) = req.api_key.as_deref() {
        set_credential(&mut cfg, &credential_slug, &display_name, token);
        if let Err(e) = config::save(&cfg).await {
            tracing::warn!("external API connection credential save failed: {e}");
        }
    }
    Json(ApiResponse::ok(view(connection, &cfg)))
}

/// PUT /api/external-api/connections/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpsertConnectionRequest>,
) -> Json<ApiResponse<ConnectionView>> {
    let display_name = req.display_name.trim().to_string();
    let mention_alias = req.mention_alias.trim().to_string();
    if display_name.is_empty() || mention_alias.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Display name and mention alias required",
        ));
    }

    let lookup_id = id.clone();
    let existing = match state
        .db
        .with_read_conn(move |conn| store::get(conn, &lookup_id))
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Connection not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("{e}"))),
    };

    // The credential slug is the store key: keep it stable across edits so a
    // rename never strands the stored token.
    let credential_slug = existing.credential_slug.clone();
    let updated = ExternalApiConnection {
        id: existing.id.clone(),
        display_name: display_name.clone(),
        mention_alias,
        endpoint: clean(req.endpoint),
        credential_slug: credential_slug.clone(),
        origin_preset: req.origin_preset,
        economy_model: clean(req.economy_model),
        default_model: clean(req.default_model),
        reasoning_model: clean(req.reasoning_model),
        created_at: existing.created_at,
        updated_at: Utc::now(),
    };

    let to_update = updated.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| store::update(conn, &to_update))
        .await
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("{e}"),
        ));
    }

    let mut cfg = state.config.write().await;
    if let Some(token) = req.api_key.as_deref() {
        set_credential(&mut cfg, &credential_slug, &display_name, token);
        if let Err(e) = config::save(&cfg).await {
            tracing::warn!("external API connection credential save failed: {e}");
        }
    }
    Json(ApiResponse::ok(view(updated, &cfg)))
}

/// DELETE /api/external-api/connections/:id — removes the row and its stored
/// credential together, so a deleted connection leaves nothing behind.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    let lookup_id = id.clone();
    let existing = match state
        .db
        .with_read_conn(move |conn| store::get(conn, &lookup_id))
        .await
    {
        Ok(Some(row)) => row,
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Connection not found",
            ))
        }
        Err(e) => return Json(ApiResponse::err(format!("{e}"))),
    };

    let slug = existing.credential_slug.clone();
    let delete_id = id.clone();
    if let Err(e) = state
        .db
        .with_conn(move |conn| store::delete(conn, &delete_id))
        .await
    {
        return Json(ApiResponse::err(format!("{e}")));
    }

    let mut cfg = state.config.write().await;
    let before = cfg.tokens.keys.len();
    cfg.tokens.keys.retain(|k| k.provider != slug);
    if cfg.tokens.keys.len() != before {
        if let Err(e) = config::save(&cfg).await {
            tracing::warn!("external API connection credential cleanup failed: {e}");
        }
    }
    Json(ApiResponse::ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_slug_is_slugified_and_prefixed() {
        let slug = credential_slug_for("Groq Fast!");
        assert!(slug.starts_with("conn-groq-fast-"), "unexpected: {slug}");
        // A blank/symbol-only alias still yields a usable slug.
        assert!(credential_slug_for("  @@  ").starts_with("conn-connection-"));
    }

    #[test]
    fn set_credential_replaces_and_clears_for_the_connection_slug() {
        let mut cfg = crate::core::config::default_config();
        set_credential(&mut cfg, "conn-groq-1234", "Groq", "sk-one");
        assert_eq!(cfg.tokens.active_key_for("conn-groq-1234"), Some("sk-one"));
        set_credential(&mut cfg, "conn-groq-1234", "Groq", "sk-two");
        assert_eq!(
            cfg.tokens
                .keys
                .iter()
                .filter(|k| k.provider == "conn-groq-1234")
                .count(),
            1
        );
        set_credential(&mut cfg, "conn-groq-1234", "Groq", "");
        assert_eq!(cfg.tokens.active_key_for("conn-groq-1234"), None);
    }
}
