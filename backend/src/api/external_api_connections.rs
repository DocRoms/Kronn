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

/// A non-persisting probe for a saved connection or the form currently being
/// edited. `api_key` is write-only; omitting it for a saved connection reuses
/// its stored credential without returning it to the browser.
#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    pub endpoint: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub connection_id: Option<String>,
    /// The saved connection's provider context. A stored credential is only
    /// reusable when this still matches the persisted connection as well as
    /// its canonical endpoint.
    #[serde(default)]
    pub origin_preset: Option<ExternalApiConnectionPreset>,
}

#[derive(Debug, Serialize)]
pub struct TestConnectionResponse {
    pub ok: bool,
    pub status: String,
    pub models: Vec<String>,
    pub hint: Option<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Canonicalize a user-entered mention alias into the BARE form persisted in
/// the DB. The UI suggests `@groq`; storing that verbatim makes
/// `connection_mention_alias` re-prepend `@` and emit `@@groq`, an alias the
/// resolver can never match. We strip a single leading `@`, trim and lowercase,
/// then reject anything empty or carrying a character that cannot appear inside
/// a mention (whitespace or a second `@`).
fn canonicalize_alias(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim();
    let bare = trimmed.strip_prefix('@').unwrap_or(trimmed).trim();
    let alias = bare.to_lowercase();
    if alias.is_empty() {
        return Err("Mention alias required");
    }
    if alias.chars().any(|c| c.is_whitespace() || c == '@') {
        return Err("Mention alias is invalid");
    }
    Ok(alias)
}

/// Normalize a base endpoint so the shared `OpenAiCodec` — which appends
/// `/v1/chat/completions` — never produces a doubled `/v1`. Operators paste the
/// URL documented by the service, which for Groq/Together ends in `/v1`; we
/// store the bare base. Returns `None` when nothing usable remains, which the
/// callers reject: a connection with no endpoint is not executable.
fn normalize_endpoint(raw: Option<String>) -> Option<String> {
    let value = raw?.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        return None;
    }
    let base = value
        .strip_suffix("/v1")
        .unwrap_or(&value)
        .trim_end_matches('/');
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

fn probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(6))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default()
}

/// Validate a connection in two bounded phases:
///
/// 1. discover the OpenAI-compatible catalogue via `GET /v1/models`;
/// 2. when a key is present and the catalogue is non-empty, confirm the key is
///    genuinely accepted with a minimal authenticated `POST /v1/chat/completions`.
///
/// Phase 2 exists because `/v1/models` is public on several providers
/// (LiteLLM, some NVIDIA/Groq deployments): a catalogue read alone lets an
/// invalid key through. The chat call is rejected with 401/403 when the key is
/// wrong, so an invalid key can no longer pass. Neither phase surfaces an
/// upstream body or the submitted key.
async fn probe_models(endpoint: &str, api_key: Option<&str>) -> TestConnectionResponse {
    let catalogue = fetch_catalogue(endpoint, api_key).await;
    if catalogue.ok {
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            if let Some(model) = catalogue.models.first() {
                if let Some(auth_failure) = probe_auth(endpoint, key, model).await {
                    return auth_failure;
                }
            }
        }
    }
    catalogue
}

/// Minimal authenticated invocation confirming the credential is accepted. A
/// `max_tokens: 1` request is the smallest billable/no-output probe compatible
/// with the OpenAI chat contract. A 2xx response confirms the credential;
/// 401/403 are classified as authentication errors, while every other HTTP
/// status is a generic probe failure because it does not prove that the
/// connection is usable. `None` = the credential passed.
async fn probe_auth(endpoint: &str, api_key: &str, model: &str) -> Option<TestConnectionResponse> {
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
    });
    let request = probe_client()
        .post(format!("{endpoint}/v1/chat/completions"))
        .bearer_auth(api_key)
        .json(&body);
    match request.send().await {
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => {
            Some(TestConnectionResponse {
                ok: false,
                status: "auth_error".into(),
                models: vec![],
                hint: Some(
                    "The endpoint rejected the credentials. Check the API key and its permissions."
                        .into(),
                ),
            })
        }
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(TestConnectionResponse {
            ok: false,
            status: "http_error".into(),
            models: vec![],
            hint: Some(format!(
                "The endpoint returned HTTP {} while validating the connection.",
                response.status().as_u16()
            )),
        }),
        Err(error) if error.is_timeout() => Some(TestConnectionResponse {
            ok: false,
            status: "timeout".into(),
            models: vec![],
            hint: Some(
                "The endpoint did not respond in time. Check its URL and availability.".into(),
            ),
        }),
        Err(_) => Some(TestConnectionResponse {
            ok: false,
            status: "transport_error".into(),
            models: vec![],
            hint: Some(
                "Kronn could not reach this endpoint. Check the URL and network access.".into(),
            ),
        }),
    }
}

async fn fetch_catalogue(endpoint: &str, api_key: Option<&str>) -> TestConnectionResponse {
    let mut request = probe_client().get(format!("{endpoint}/v1/models"));
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<serde_json::Value>().await {
                Ok(body) if model_ids_from_body(&body).is_some() => {
                    let models = model_ids_from_body(&body).expect("checked above");
                    TestConnectionResponse {
                    ok: true,
                    status: "success".into(),
                    hint: models.is_empty().then(|| "The endpoint responded but returned no usable models. Check this account or endpoint.".into()),
                    models,
                    }
                }
                _ => TestConnectionResponse {
                    ok: false,
                    status: "invalid_catalogue".into(),
                    models: vec![],
                    hint: Some(
                        "The endpoint responded, but its model catalogue is not OpenAI-compatible. Check the endpoint and provider settings."
                            .into(),
                    ),
                },
            }
        }
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => TestConnectionResponse {
            ok: false,
            status: "auth_error".into(),
            models: vec![],
            hint: Some(
                "The endpoint rejected the credentials. Check the API key and its permissions."
                    .into(),
            ),
        },
        Ok(response) => TestConnectionResponse {
            ok: false,
            status: "http_error".into(),
            models: vec![],
            hint: Some(format!(
                "The endpoint returned HTTP {} while loading models.",
                response.status().as_u16()
            )),
        },
        Err(error) if error.is_timeout() => TestConnectionResponse {
            ok: false,
            status: "timeout".into(),
            models: vec![],
            hint: Some(
                "The endpoint did not respond in time. Check its URL and availability.".into(),
            ),
        },
        Err(_) => TestConnectionResponse {
            ok: false,
            status: "transport_error".into(),
            models: vec![],
            hint: Some(
                "Kronn could not reach this endpoint. Check the URL and network access.".into(),
            ),
        },
    }
}

fn model_ids_from_body(body: &serde_json::Value) -> Option<Vec<String>> {
    body["data"].as_array().and_then(|items| {
        items
            .iter()
            .map(|model| {
                model["id"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                    .map(str::to_string)
            })
            .collect()
    })
}

/// POST /api/external-api/connections/test
///
/// Validate the OpenAI-compatible models endpoint without saving form data.
/// The response contains only model ids and generic actionable status text,
/// never a submitted key or an upstream response body.
pub async fn test(
    State(state): State<AppState>,
    Json(req): Json<TestConnectionRequest>,
) -> Json<ApiResponse<TestConnectionResponse>> {
    let Some(endpoint) = normalize_endpoint(req.endpoint) else {
        return Json(ApiResponse::ok(TestConnectionResponse {
            ok: false,
            status: "invalid_url".into(),
            models: vec![],
            hint: Some("Enter a valid endpoint before testing the connection.".into()),
        }));
    };
    if reqwest::Url::parse(&endpoint).is_err() {
        return Json(ApiResponse::ok(TestConnectionResponse {
            ok: false,
            status: "invalid_url".into(),
            models: vec![],
            hint: Some("Enter a valid endpoint before testing the connection.".into()),
        }));
    }

    let stored_key = if req.api_key.is_none() {
        match req.connection_id.as_deref() {
            Some(connection_id) => {
                let lookup_id = connection_id.to_string();
                match state
                    .db
                    .with_read_conn(move |conn| store::get(conn, &lookup_id))
                    .await
                {
                    Ok(Some(connection))
                        if connection.endpoint.as_deref() == Some(endpoint.as_str())
                            && req.origin_preset == Some(connection.origin_preset) =>
                    {
                        state
                            .config
                            .read()
                            .await
                            .tokens
                            .active_key_for(&connection.credential_slug)
                            .map(str::to_string)
                    }
                    Ok(Some(_)) => {
                        return Json(ApiResponse::ok(TestConnectionResponse {
                            ok: false,
                            status: "credential_required".into(),
                            models: vec![],
                            hint: Some(
                                "The endpoint or provider changed. Enter the API key again before testing."
                                    .into(),
                            ),
                        }));
                    }
                    _ => None,
                }
            }
            None => None,
        }
    } else {
        None
    };
    let key = req
        .api_key
        .as_deref()
        .or(stored_key.as_deref())
        .filter(|key| !key.trim().is_empty());
    Json(ApiResponse::ok(probe_models(&endpoint, key).await))
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
    if display_name.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Display name required",
        ));
    }
    let mention_alias = match canonicalize_alias(&req.mention_alias) {
        Ok(alias) => alias,
        Err(msg) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg)),
    };
    let endpoint = match normalize_endpoint(req.endpoint) {
        Some(endpoint) => endpoint,
        None => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "Endpoint required",
            ))
        }
    };

    let now = Utc::now();
    let credential_slug = credential_slug_for(&mention_alias);
    let connection = ExternalApiConnection {
        id: uuid::Uuid::new_v4().to_string(),
        display_name: display_name.clone(),
        mention_alias,
        endpoint: Some(endpoint),
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
    if display_name.is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Display name required",
        ));
    }
    let mention_alias = match canonicalize_alias(&req.mention_alias) {
        Ok(alias) => alias,
        Err(msg) => return Json(ApiResponse::err_coded(ApiErrorCode::Validation, msg)),
    };
    let endpoint = match normalize_endpoint(req.endpoint) {
        Some(endpoint) => endpoint,
        None => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                "Endpoint required",
            ))
        }
    };

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
        endpoint: Some(endpoint),
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
    fn canonicalize_alias_strips_leading_at_and_normalizes() {
        // The UI suggests `@groq`; the bare, stored form must not carry the `@`,
        // otherwise `connection_mention_alias` re-prepends it into `@@groq`.
        assert_eq!(canonicalize_alias("@groq"), Ok("groq".to_string()));
        assert_eq!(canonicalize_alias("  @Groq  "), Ok("groq".to_string()));
        assert_eq!(canonicalize_alias("groq"), Ok("groq".to_string()));
        assert_eq!(
            canonicalize_alias("Together-AI"),
            Ok("together-ai".to_string())
        );
    }

    #[test]
    fn canonicalize_alias_rejects_empty_and_invalid() {
        assert!(canonicalize_alias("").is_err());
        assert!(canonicalize_alias("   ").is_err());
        assert!(canonicalize_alias("@").is_err());
        // A second `@` or embedded whitespace can never appear in a mention.
        assert!(canonicalize_alias("@@groq").is_err());
        assert!(canonicalize_alias("gr oq").is_err());
    }

    #[test]
    fn canonical_alias_round_trips_to_a_single_at_mention() {
        // Create/update path: canonicalized alias -> stored row -> the mention
        // the resolver actually looks for. The regression the review asked for:
        // a UI-suggested `@groq` yields `@groq`, never `@@groq`.
        let alias = canonicalize_alias("@groq").unwrap();
        let connection = ExternalApiConnection {
            id: "id".into(),
            display_name: "Groq".into(),
            mention_alias: alias,
            endpoint: Some("https://api.groq.com/openai".into()),
            credential_slug: "conn-groq-1234".into(),
            origin_preset: ExternalApiConnectionPreset::Other,
            economy_model: None,
            default_model: None,
            reasoning_model: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        assert_eq!(
            crate::db::external_api_connections::connection_mention_alias(&connection),
            "@groq"
        );
    }

    #[test]
    fn normalize_endpoint_strips_trailing_v1_and_slashes() {
        assert_eq!(
            normalize_endpoint(Some("https://api.together.xyz/v1".into())),
            Some("https://api.together.xyz".into())
        );
        assert_eq!(
            normalize_endpoint(Some("https://api.groq.com/openai/v1/".into())),
            Some("https://api.groq.com/openai".into())
        );
        // A base without `/v1` (NVIDIA/LiteLLM) is preserved verbatim.
        assert_eq!(
            normalize_endpoint(Some("https://integrate.api.nvidia.com".into())),
            Some("https://integrate.api.nvidia.com".into())
        );
        // Blank/whitespace -> None, which the handlers reject as "Endpoint required".
        assert_eq!(normalize_endpoint(Some("   ".into())), None);
        assert_eq!(normalize_endpoint(None), None);
    }

    #[test]
    fn normalized_endpoint_yields_the_correct_final_chat_url() {
        // The third-service regression: a documented base URL ending in `/v1`
        // must resolve to exactly one `/v1/chat/completions`, not two.
        use crate::agents::chat_codec::{ChatCodec, OpenAiCodec};
        let stored = normalize_endpoint(Some("https://api.together.xyz/v1".into())).unwrap();
        assert_eq!(
            OpenAiCodec.endpoint(&stored),
            "https://api.together.xyz/v1/chat/completions"
        );
    }

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

    #[test]
    fn model_catalogue_accepts_a_valid_empty_list_and_rejects_invalid_shapes() {
        let models = model_ids_from_body(&serde_json::json!({
            "data": [{"id": "model-a"}, {"id": "model-b"}]
        }));
        assert_eq!(models, Some(vec!["model-a".into(), "model-b".into()]));
        assert_eq!(
            model_ids_from_body(&serde_json::json!({"data": []})),
            Some(vec![])
        );
        assert_eq!(model_ids_from_body(&serde_json::json!({})), None);
        assert_eq!(
            model_ids_from_body(&serde_json::json!({"data": [{"name": "missing-id"}]})),
            None
        );
    }
}
