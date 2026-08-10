//! LiteLLM proxy endpoints.
//!
//! LiteLLM is the second server-shaped agent after Ollama, but unlike Ollama
//! its proxy can live anywhere and may be behind a key, so the endpoint is the
//! user's to declare rather than something to guess. The settings card is
//! therefore two-step: connect (endpoint + key, validated by `test`), then
//! pick models. `test` only persists once the probe succeeds — a saved config
//! that does not answer would strand the card in a broken state.
//!
//! Execution goes through the shared HTTP chat path in `runner.rs` with
//! `OpenAiCodec`; these endpoints serve the card.

use crate::core::config;
use crate::models::*;
use crate::AppState;
use axum::{extract::State, Json};

/// Provider slug under which the proxy key lives in the encrypted token store.
const PROVIDER: &str = "litellm";
const MODEL_NOT_IN_CATALOGUE: &str = "kronn:model-not-in-catalogue";

/// Public accessor for the runner, which already holds the saved endpoint and
/// only needs the fallback chain applied.
pub fn resolve_base_url_pub(stored: Option<&str>) -> String {
    resolve_base_url(stored)
}

async fn lite_llm_base_url(state: &AppState) -> String {
    let cfg = state.config.read().await;
    resolve_base_url(cfg.agents.lite_llm.base_url.as_deref())
}

async fn lite_llm_api_key(state: &AppState) -> Option<String> {
    let cfg = state.config.read().await;
    cfg.tokens
        .active_key_for(PROVIDER)
        .filter(|k| !k.trim().is_empty())
        .map(str::to_string)
}

/// Normalise a user-typed endpoint: add a scheme, drop a trailing slash.
/// Priority: stored config > `LITELLM_BASE_URL` > Docker heuristic > the
/// documented default proxy port.
fn resolve_base_url(stored: Option<&str>) -> String {
    if let Some(url) = stored.map(str::trim).filter(|u| !u.is_empty()) {
        return normalize_url(url);
    }
    if let Ok(env) = std::env::var("LITELLM_BASE_URL") {
        let env = env.trim();
        if !env.is_empty() && env != "0.0.0.0" {
            return normalize_url(env);
        }
    }
    if crate::core::env::is_docker() {
        "http://host.docker.internal:4000".to_string()
    } else {
        "http://localhost:4000".to_string()
    }
}

fn normalize_url(raw: &str) -> String {
    let raw = raw.trim().trim_end_matches('/');
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        format!("http://{}", raw)
    }
}

fn client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        // Bound connect separately so a black-holed host fails fast instead of
        // sitting for the whole request timeout.
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default()
}

/// One `GET /v1/models` probe. Returns the declared models, or an error string
/// already phrased for the user.
async fn probe(base: &str, api_key: Option<&str>) -> Result<Vec<LiteLlmModel>, (String, String)> {
    let mut req = client(6).get(format!("{}/v1/models", base));
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let mut models: Vec<LiteLlmModel> = body["data"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            Some(LiteLlmModel {
                                id: m["id"].as_str()?.to_string(),
                                backing_model: None,
                                provider: None,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            // `/v1/models` only gives aliases. `/model/info` discloses what
            // actually serves each one, which is the difference between "you
            // have a model" and "you know which machine answers".
            enrich_with_backing_models(base, api_key, &mut models).await;
            Ok(models)
        }
        // Something IS listening — almost always auth, which is a different
        // fix from "not running". "No key sent" and "key rejected" are also
        // different fixes, and a corporate proxy answers 401 to both: saying
        // "check your key" to someone who has not entered one sends them
        // hunting for a problem that isn't there.
        Ok(resp) if resp.status().as_u16() == 401 || resp.status().as_u16() == 403 => {
            let sent_key = api_key.is_some_and(|k| !k.trim().is_empty());
            let detail = resp
                .json::<serde_json::Value>()
                .await
                .ok()
                .and_then(|b| b["error"]["message"].as_str().map(str::to_string));
            let base_msg = if sent_key {
                "Le proxy a refusé cette clé (401/403). Vérifiez qu'elle est valide et \
                 autorisée sur cet endpoint."
            } else {
                "Ce proxy exige une clé : saisissez-la dans le champ « Clé » ci-dessus. \
                 (Sa page d'accueil reste ouverte sans clé, seules les routes /v1/* sont \
                 protégées.)"
            };
            Err((
                "unauthorized".into(),
                match detail {
                    Some(d) => format!("{base_msg}\n\nRéponse du proxy : {d}"),
                    None => base_msg.to_string(),
                },
            ))
        }
        Ok(resp) => Err((
            "unreachable".into(),
            format!("Le proxy a répondu {} sur /v1/models.", resp.status()),
        )),
        Err(e) if e.is_timeout() || e.is_connect() => Err((
            if which::which("litellm").is_ok() {
                "offline".into()
            } else {
                "not_installed".into()
            },
            if which::which("litellm").is_ok() {
                "Aucune réponse. Le proxy est-il lancé ? litellm --config <config.yaml>".into()
            } else {
                "LiteLLM n'est pas installé. Kronn peut l'installer (Installer ci-dessus).".into()
            },
        )),
        Err(e) => Err(("offline".into(), format!("Connexion impossible : {}", e))),
    }
}

/// Ask the proxy which model backs each alias. Best-effort: `/model/info` is
/// optional and often admin-gated, and an alias list without provenance is
/// still usable.
async fn enrich_with_backing_models(
    base: &str,
    api_key: Option<&str>,
    models: &mut [LiteLlmModel],
) {
    let mut req = client(5).get(format!("{}/model/info", base));
    if let Some(key) = api_key.filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(key);
    }
    let Ok(resp) = req.send().await else { return };
    if !resp.status().is_success() {
        return;
    }
    let Ok(body) = resp.json::<serde_json::Value>().await else {
        return;
    };
    let Some(entries) = body["data"].as_array() else {
        return;
    };
    for m in models.iter_mut() {
        let Some(entry) = entries.iter().find(|e| e["model_name"] == m.id.as_str()) else {
            continue;
        };
        let Some(backing) = entry["litellm_params"]["model"].as_str() else {
            continue;
        };
        // LiteLLM spells the route as `<provider>/<model>`; a bare name means
        // the provider is implicit (OpenAI), so claim nothing.
        m.provider = backing
            .split_once('/')
            .map(|(p, _)| p.trim_end_matches("_chat").to_string());
        m.backing_model = Some(backing.to_string());
    }
}

/// A cheap real invocation, unlike `/v1/models`, proves that the proxy's
/// upstream project, region and entitlements can actually serve the alias.
async fn probe_model(base: &str, api_key: Option<&str>, model: &str) -> Result<(), (u16, String)> {
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Reply only: OK" }],
        "max_tokens": 16,
        "temperature": 0,
        "stream": false,
    });
    let mut request = client(45)
        .post(format!("{base}/v1/chat/completions"))
        .json(&body);
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => Ok(()),
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            Err((status, body))
        }
        Err(error) => Err((503, format!("LiteLLM unreachable: {error}"))),
    }
}

#[derive(Debug, PartialEq)]
enum ModelRetryProbe {
    Healthy,
    NotInCatalogue,
    ModelFailed(u16, String),
    CatalogueUnavailable(String),
}

async fn probe_model_for_retry(base: &str, api_key: Option<&str>, model: &str) -> ModelRetryProbe {
    let catalogue = match probe(base, api_key).await {
        Ok(models) => models,
        Err((status, hint)) => {
            return ModelRetryProbe::CatalogueUnavailable(format!("{status}: {hint}"));
        }
    };
    if !catalogue.iter().any(|candidate| candidate.id == model) {
        return ModelRetryProbe::NotInCatalogue;
    }
    match probe_model(base, api_key, model).await {
        Ok(()) => ModelRetryProbe::Healthy,
        Err((status, error)) => ModelRetryProbe::ModelFailed(status, error),
    }
}

/// POST /api/lite-llm/test
///
/// Validate an endpoint + key pair and, only on success, persist it. The card
/// gates model selection on `saved`.
pub async fn test(
    State(state): State<AppState>,
    Json(req): Json<LiteLlmTestRequest>,
) -> Json<ApiResponse<LiteLlmTestResponse>> {
    let base = normalize_url(&req.base_url);
    if base == "http://" {
        return Json(ApiResponse::err("Endpoint requis"));
    }

    // An omitted key means "keep what is stored"; an empty one means "clear".
    let effective_key: Option<String> = match req.api_key.as_deref() {
        Some(k) if k.trim().is_empty() => None,
        Some(k) => Some(k.to_string()),
        None => lite_llm_api_key(&state).await,
    };

    match probe(&base, effective_key.as_deref()).await {
        Ok(models) => {
            let mut cfg = state.config.write().await;
            cfg.agents.lite_llm.base_url = Some(base.clone());
            // Only touch the store when the caller actually sent a key, so a
            // re-test from the card doesn't wipe a working credential.
            if let Some(k) = req.api_key.as_deref() {
                upsert_key(&mut cfg, k);
            }
            let saved = match config::save(&cfg).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("LiteLLM config save failed: {}", e);
                    false
                }
            };
            let hint = if models.is_empty() {
                Some(
                    "Le proxy répond mais ne déclare aucun modèle. Ajoutez-en un dans son \
                     config.yaml, puis relancez-le."
                        .to_string(),
                )
            } else if !saved {
                Some("Connexion OK mais l'enregistrement a échoué.".to_string())
            } else {
                None
            };
            Json(ApiResponse::ok(LiteLlmTestResponse {
                ok: true,
                saved,
                status: "online".into(),
                endpoint: base,
                models,
                hint,
            }))
        }
        Err((status, hint)) => Json(ApiResponse::ok(LiteLlmTestResponse {
            ok: false,
            saved: false,
            status,
            endpoint: base,
            models: vec![],
            hint: Some(hint),
        })),
    }
}

/// Replace the active LiteLLM key, or clear it when `value` is blank.
fn upsert_key(cfg: &mut AppConfig, value: &str) {
    cfg.tokens.keys.retain(|k| k.provider != PROVIDER);
    if value.trim().is_empty() {
        return;
    }
    cfg.tokens.keys.push(ApiKey {
        id: uuid::Uuid::new_v4().to_string(),
        name: "LiteLLM proxy".into(),
        provider: PROVIDER.into(),
        value: value.to_string(),
        active: true,
    });
}

/// GET /api/lite-llm/health
pub async fn health(State(state): State<AppState>) -> Json<ApiResponse<LiteLlmHealthResponse>> {
    let base = lite_llm_base_url(&state).await;
    let key = lite_llm_api_key(&state).await;
    let configured = {
        let cfg = state.config.read().await;
        cfg.agents.lite_llm.base_url.is_some()
    };

    match probe(&base, key.as_deref()).await {
        Ok(models) => {
            let hint = if models.is_empty() {
                Some("Le proxy répond mais ne déclare aucun modèle.".into())
            } else {
                None
            };
            Json(ApiResponse::ok(LiteLlmHealthResponse {
                status: "online".into(),
                endpoint: base,
                models_count: models.len() as u32,
                hint,
                configured,
            }))
        }
        Err((status, hint)) => Json(ApiResponse::ok(LiteLlmHealthResponse {
            status,
            endpoint: base,
            models_count: 0,
            hint: Some(hint),
            configured,
        })),
    }
}

/// GET /api/lite-llm/models
///
/// Whatever the operator declared in `config.yaml` is the truth — unlike
/// Ollama there is no local registry to fall back on.
pub async fn models(State(state): State<AppState>) -> Json<ApiResponse<LiteLlmModelsResponse>> {
    let base = lite_llm_base_url(&state).await;
    let key = lite_llm_api_key(&state).await;
    let models = probe(&base, key.as_deref()).await.unwrap_or_default();
    Json(ApiResponse::ok(LiteLlmModelsResponse { models }))
}

/// GET /api/lite-llm/model-failures — failures for the currently configured
/// endpoint only. Repointing LiteLLM never leaks stale warnings from the old
/// proxy into the new card.
pub async fn model_failures(
    State(state): State<AppState>,
) -> Json<ApiResponse<LiteLlmModelFailuresResponse>> {
    let endpoint = lite_llm_base_url(&state).await;
    let failures = state
        .db
        .with_read_conn(move |conn| Ok(crate::db::lite_llm_model_failures::list(conn, &endpoint)?))
        .await
        .unwrap_or_default();
    Json(ApiResponse::ok(LiteLlmModelFailuresResponse { failures }))
}

/// DELETE /api/lite-llm/model-failures — forget one diagnostic for the
/// currently configured endpoint. A future runtime failure records it again.
pub async fn forget_model_failure(
    State(state): State<AppState>,
    Json(request): Json<LiteLlmModelRetryRequest>,
) -> Json<ApiResponse<bool>> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        return Json(ApiResponse::err("Model required"));
    }
    let endpoint = lite_llm_base_url(&state).await;
    let cleared = state
        .db
        .with_conn(move |conn| {
            Ok(crate::db::lite_llm_model_failures::clear(
                conn, &endpoint, &model,
            )?)
        })
        .await
        .unwrap_or(false);
    Json(ApiResponse::ok(cleared))
}

/// POST /api/lite-llm/model-failures/retry — verify the alias is still in the
/// proxy catalogue, then run a real minimal completion. A healthy response
/// clears the warning; a failure refreshes its diagnostic.
pub async fn retry_model(
    State(state): State<AppState>,
    Json(request): Json<LiteLlmModelRetryRequest>,
) -> Json<ApiResponse<LiteLlmModelRetryResponse>> {
    let model = request.model.trim().to_string();
    if model.is_empty() {
        return Json(ApiResponse::err("Model required"));
    }
    let endpoint = lite_llm_base_url(&state).await;
    let key = lite_llm_api_key(&state).await;
    match probe_model_for_retry(&endpoint, key.as_deref(), &model).await {
        ModelRetryProbe::Healthy => {
            let endpoint_for_db = endpoint.clone();
            let model_for_db = model.clone();
            let _ = state
                .db
                .with_conn(move |conn| {
                    Ok(crate::db::lite_llm_model_failures::clear(
                        conn,
                        &endpoint_for_db,
                        &model_for_db,
                    )?)
                })
                .await;
            Json(ApiResponse::ok(LiteLlmModelRetryResponse {
                healthy: true,
                failure: None,
            }))
        }
        ModelRetryProbe::CatalogueUnavailable(error) => Json(ApiResponse::err(error)),
        outcome => {
            let (status, error) = match outcome {
                ModelRetryProbe::NotInCatalogue => (410, MODEL_NOT_IN_CATALOGUE.to_string()),
                ModelRetryProbe::ModelFailed(status, error) => (status, error),
                ModelRetryProbe::Healthy | ModelRetryProbe::CatalogueUnavailable(_) => {
                    unreachable!()
                }
            };
            let endpoint_for_db = endpoint.clone();
            let model_for_db = model.clone();
            let error_for_db = error.clone();
            let _ = state
                .db
                .with_conn(move |conn| {
                    crate::db::lite_llm_model_failures::record(
                        conn,
                        &endpoint_for_db,
                        &model_for_db,
                        status,
                        &error_for_db,
                    )?;
                    Ok(())
                })
                .await;
            let endpoint_for_db = endpoint.clone();
            let model_for_db = model.clone();
            let failure = state
                .db
                .with_read_conn(move |conn| {
                    Ok(
                        crate::db::lite_llm_model_failures::list(conn, &endpoint_for_db)?
                            .into_iter()
                            .find(|failure| failure.model == model_for_db),
                    )
                })
                .await
                .ok()
                .flatten();
            Json(ApiResponse::ok(LiteLlmModelRetryResponse {
                healthy: false,
                failure,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn stored_endpoint_wins_and_is_normalised() {
        assert_eq!(
            resolve_base_url(Some("proxy.internal:4000/")),
            "http://proxy.internal:4000"
        );
        assert_eq!(
            resolve_base_url(Some("https://proxy.internal")),
            "https://proxy.internal"
        );
    }

    #[test]
    #[serial]
    fn blank_stored_endpoint_falls_through_to_the_default_port() {
        let prev = std::env::var("LITELLM_BASE_URL").ok();
        std::env::remove_var("LITELLM_BASE_URL");
        for stored in [None, Some(""), Some("   ")] {
            assert!(
                resolve_base_url(stored).ends_with(":4000"),
                "unexpected for {stored:?}: {}",
                resolve_base_url(stored)
            );
        }
        if let Some(p) = prev {
            std::env::set_var("LITELLM_BASE_URL", p);
        }
    }

    #[test]
    fn upsert_key_replaces_and_clearing_removes() {
        let mut cfg = crate::core::config::default_config();
        upsert_key(&mut cfg, "sk-one");
        assert_eq!(cfg.tokens.active_key_for(PROVIDER), Some("sk-one"));
        // A second write must not leave the first key behind.
        upsert_key(&mut cfg, "sk-two");
        assert_eq!(
            cfg.tokens
                .keys
                .iter()
                .filter(|k| k.provider == PROVIDER)
                .count(),
            1
        );
        assert_eq!(cfg.tokens.active_key_for(PROVIDER), Some("sk-two"));
        upsert_key(&mut cfg, "");
        assert_eq!(cfg.tokens.active_key_for(PROVIDER), None);
    }

    #[tokio::test]
    async fn model_probe_is_a_real_minimal_completion_with_auth() {
        use wiremock::matchers::{body_partial_json, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer proxy-key"))
            .and(body_partial_json(serde_json::json!({
                "model": "model-a",
                "stream": false,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{ "message": { "content": "OK" } }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            probe_model(&server.uri(), Some("proxy-key"), "model-a").await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn model_probe_preserves_upstream_status_and_diagnostic() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(404).set_body_string("publisher model missing"))
            .mount(&server)
            .await;

        let failure = probe_model(&server.uri(), None, "missing-model")
            .await
            .expect_err("404 must stay unhealthy");
        assert_eq!(failure.0, 404);
        assert_eq!(failure.1, "publisher model missing");
    }

    #[tokio::test]
    async fn retry_probe_does_not_invoke_a_model_removed_from_the_catalogue() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "id": "replacement-model" }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        assert_eq!(
            probe_model_for_retry(&server.uri(), None, "disabled-model").await,
            ModelRetryProbe::NotInCatalogue
        );
    }
}
