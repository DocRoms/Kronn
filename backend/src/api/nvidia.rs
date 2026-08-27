//! NVIDIA-hosted models (KT-337).
//!
//! Same wire contract as the LiteLLM card — `{base}/v1/models`,
//! `{base}/v1/chat/completions`, bearer auth — so the runner reuses `OpenAiCodec`
//! and the whole HTTP path unchanged. Two things differ, and they are the reason
//! this module exists rather than a second endpoint field on the LiteLLM card:
//!
//! 1. **The endpoint is the hosted service, not an operator's proxy.** There is
//!    nothing to install and no port to guess, so the default is the public
//!    endpoint and `NVIDIA_BASE_URL` is the only override (a self-hosted NIM
//!    container exposes the same contract, which is why it stays configurable).
//! 2. **The catalogue is NOT the entitlement list.** `GET /v1/models` answers 200
//!    *without a key* and lists every model the service knows — including ones
//!    this account may not call (`404 … not found for account`), ones past their
//!    end of life (`410`), and ones that simply never answer. Measured on the
//!    real service: 102 ids, 25 vendors, and several of each failure class. So a
//!    model is only ever trusted after a real probe has answered, never because
//!    it appeared in a list.

use crate::models::*;
use crate::AppState;
use axum::{extract::State, Json};

/// Credential slot in `TokensConfig`. Kept distinct from `litellm` so one
/// provider can never borrow the other's key.
pub const PROVIDER: &str = "nvidia";

/// The hosted OpenAI-compatible endpoint. Deliberately without the `/v1` suffix:
/// the shared codec appends `/v1/chat/completions` and the probes append
/// `/v1/models`, exactly as they do for a LiteLLM proxy.
const DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com";

/// Public accessor for the runner, which holds the saved endpoint (if any) but
/// not this module's fallback chain.
pub fn resolve_base_url_pub(stored: Option<&str>) -> String {
    resolve_base_url(stored)
}

/// Saved endpoint → `NVIDIA_BASE_URL` → the hosted default. A trailing slash is
/// dropped and a bare host gains a scheme, so a pasted value behaves.
fn resolve_base_url(stored: Option<&str>) -> String {
    let raw = stored
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("NVIDIA_BASE_URL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    normalize_url(&raw)
}

fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        // A hosted endpoint is never plain HTTP; assume TLS rather than
        // downgrading a pasted host silently.
        format!("https://{trimmed}")
    }
}

fn client(timeout_secs: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .unwrap_or_default()
}

/// What a real invocation says about one model id. The catalogue cannot answer
/// this (see the module header), so the UI asks per model and remembers.
#[derive(Debug, PartialEq)]
pub enum ModelProbe {
    /// The model answered: it is safe to assign to a tier.
    Usable,
    /// The account is not entitled to it (`404 … not found for account`).
    NotEntitled,
    /// The service retired it (`410`).
    Retired,
    /// It answered with another error status.
    Refused { status: u16, detail: String },
    /// It never answered within the deadline — a cold start or a dead route.
    /// Deliberately distinct from a refusal: retrying may work, so the UI must
    /// not present it as "you cannot use this".
    NoAnswer,
}

/// A cheap real invocation, the only trustworthy availability signal. Mirrors
/// LiteLLM's `probe_model` reasoning: the catalogue proves a name exists, this
/// proves the account can actually be served.
pub async fn probe_model(base: &str, api_key: Option<&str>, model: &str) -> ModelProbe {
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
        Ok(response) if response.status().is_success() => ModelProbe::Usable,
        Ok(response) => {
            let status = response.status().as_u16();
            let detail = response.text().await.unwrap_or_default();
            match status {
                // The service distinguishes these two, and so must we: one is a
                // permanent dead end, the other may be fixed by a plan change.
                404 => ModelProbe::NotEntitled,
                410 => ModelProbe::Retired,
                _ => ModelProbe::Refused { status, detail },
            }
        }
        Err(error) if error.is_timeout() => ModelProbe::NoAnswer,
        Err(error) => ModelProbe::Refused {
            status: 503,
            detail: format!("NVIDIA unreachable: {error}"),
        },
    }
}

/// The catalogue, as the service reports it. No key is required (the listing is
/// public), which is exactly why its output must never be presented as "models
/// you can use".
pub async fn probe_catalogue(base: &str, api_key: Option<&str>) -> Result<Vec<String>, String> {
    let mut request = client(15).get(format!("{base}/v1/models"));
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Endpoint NVIDIA injoignable : {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "L'endpoint a répondu {} sur /v1/models.",
            response.status()
        ));
    }
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("Réponse /v1/models illisible : {error}"))?;
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    Ok(ids)
}

/// Map an internal probe outcome to the wire verdict + a message written for a
/// human. The upstream body is deliberately NOT forwarded verbatim: NVIDIA's 404
/// includes an account identifier, which has no business in a settings card.
fn verdict_of(probe: &ModelProbe) -> (NvidiaProbeVerdict, String) {
    match probe {
        ModelProbe::Usable => (
            NvidiaProbeVerdict::Usable,
            "Le modèle a répondu : utilisable.".into(),
        ),
        ModelProbe::NotEntitled => (
            NvidiaProbeVerdict::NotEntitled,
            "Ce modèle est au catalogue du service mais pas accessible à ce compte.".into(),
        ),
        ModelProbe::Retired => (
            NvidiaProbeVerdict::Retired,
            "Ce modèle est en fin de vie côté NVIDIA.".into(),
        ),
        ModelProbe::Refused { status, .. } => (
            NvidiaProbeVerdict::Refused,
            format!("Refusé par le service (HTTP {status})."),
        ),
        ModelProbe::NoAnswer => (
            NvidiaProbeVerdict::NoAnswer,
            "Aucune réponse dans le délai. Peut être un démarrage à froid : réessayez.".into(),
        ),
    }
}

async fn stored_endpoint_and_key(state: &AppState) -> (String, Option<String>) {
    let config = state.config.read().await;
    let stored = config
        .agents
        .nvidia
        .base_url
        .clone()
        .filter(|value| !value.trim().is_empty());
    let key = config
        .tokens
        .active_key_for(PROVIDER)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty());
    (resolve_base_url(stored.as_deref()), key)
}

/// The catalogue. Returns `has_key` alongside it precisely because the listing
/// succeeds WITHOUT a key: without that flag a keyless setup looks healthy right
/// up to the first real call.
pub async fn models(State(state): State<AppState>) -> Json<ApiResponse<NvidiaModelsResponse>> {
    let (endpoint, key) = stored_endpoint_and_key(&state).await;
    match probe_catalogue(&endpoint, key.as_deref()).await {
        Err(error) => Json(ApiResponse::err(error)),
        Ok(ids) => {
            let models = ids
                .into_iter()
                .map(|id| {
                    let vendor = id.split('/').next().unwrap_or("").to_string();
                    NvidiaModel {
                        id,
                        vendor,
                        probe: None,
                    }
                })
                .collect();
            Json(ApiResponse::ok(NvidiaModelsResponse {
                models,
                endpoint,
                has_key: key.is_some(),
            }))
        }
    }
}

/// Probe ONE model with a real minimal invocation. This is the only trustworthy
/// availability signal for this provider, so the card calls it before letting a
/// model be assigned to a tier.
pub async fn probe(
    State(state): State<AppState>,
    Json(req): Json<NvidiaProbeRequest>,
) -> Json<ApiResponse<NvidiaProbeResponse>> {
    let model = req.model.trim().to_string();
    if model.is_empty() {
        return Json(ApiResponse::err("model is required"));
    }
    let (endpoint, key) = stored_endpoint_and_key(&state).await;
    if key.is_none() {
        return Json(ApiResponse::err(
            "Aucune clé NVIDIA enregistrée : ajoutez-la avant de vérifier un modèle.",
        ));
    }
    let outcome = probe_model(&endpoint, key.as_deref(), &model).await;
    let (verdict, detail) = verdict_of(&outcome);
    Json(ApiResponse::ok(NvidiaProbeResponse {
        model,
        verdict,
        detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_defaults_to_the_hosted_endpoint_without_a_v1_suffix() {
        // The shared codec appends `/v1/chat/completions`, so a `/v1` here would
        // produce `/v1/v1/...`. This is the most likely configuration mistake.
        let base = resolve_base_url(None);
        assert_eq!(base, "https://integrate.api.nvidia.com");
        assert!(
            !base.ends_with("/v1"),
            "the /v1 segment belongs to the codec"
        );
    }

    #[test]
    fn stored_endpoint_wins_and_is_normalised() {
        // A self-hosted NIM container exposes the same contract on another host,
        // which is why the endpoint stays configurable.
        assert_eq!(
            resolve_base_url(Some("http://localhost:8000/")),
            "http://localhost:8000"
        );
        // A bare host is assumed to be TLS: a hosted endpoint is never plain HTTP.
        assert_eq!(
            resolve_base_url(Some("integrate.api.nvidia.com")),
            "https://integrate.api.nvidia.com"
        );
        // Blank is not a value: fall through to the default.
        assert_eq!(resolve_base_url(Some("   ")), DEFAULT_BASE_URL);
    }

    #[tokio::test]
    async fn catalogue_reads_the_openai_shape_and_authenticates_with_a_bearer() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer nvapi-probe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [
                    { "id": "meta/llama-3.1-8b-instruct" },
                    { "id": "deepseek-ai/deepseek-r1" },
                    { "id": 42 },
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let ids = probe_catalogue(&server.uri(), Some("nvapi-probe"))
            .await
            .expect("a NVIDIA-shaped catalogue must parse");
        // Sorted, and an entry whose id is not a string is dropped rather than
        // crashing the whole listing.
        assert_eq!(
            ids,
            vec![
                "deepseek-ai/deepseek-r1".to_string(),
                "meta/llama-3.1-8b-instruct".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn catalogue_succeeds_on_an_endpoint_without_the_litellm_model_info_route() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // `/model/info` is LiteLLM's own enrichment route; NVIDIA has no such
        // thing. Only `/v1/models` is mounted here, so any request to
        // `/model/info` would 404 and surface as a failed listing.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [{ "id": "nvidia/nemotron-4-340b-instruct" }]
            })))
            .mount(&server)
            .await;

        let ids = probe_catalogue(&server.uri(), None)
            .await
            .expect("the listing must not depend on a LiteLLM-only route");
        assert_eq!(ids, vec!["nvidia/nemotron-4-340b-instruct".to_string()]);

        // Prove the absence is structural, not luck: the module never asks for
        // that route at all.
        let asked_for_model_info = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|request| request.url.path().contains("model/info"));
        assert!(
            !asked_for_model_info,
            "the NVIDIA card must never request LiteLLM's /model/info"
        );
    }

    #[tokio::test]
    async fn a_refusal_never_leaks_the_upstream_account_identifier_or_the_key() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // NVIDIA's real 404 body names the account the request was billed to.
        // That identifier belongs in no settings card, so the verdict is built
        // from the status alone.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(404)
                    .set_body_string("model not found for account acct-7f3c-BILLING-IDENTIFIER"),
            )
            .mount(&server)
            .await;

        let outcome = probe_model(&server.uri(), Some("nvapi-SECRET-KEY"), "meta/absent").await;
        assert_eq!(outcome, ModelProbe::NotEntitled);

        let (verdict, detail) = verdict_of(&outcome);
        assert_eq!(verdict, NvidiaProbeVerdict::NotEntitled);
        assert!(
            !detail.contains("acct-7f3c-BILLING-IDENTIFIER"),
            "the account identifier must not reach the card: {detail}"
        );
        assert!(
            !detail.contains("nvapi-SECRET-KEY"),
            "the api key must never appear in a verdict: {detail}"
        );
    }

    #[tokio::test]
    async fn a_retired_model_is_not_reported_as_unavailable_to_this_account() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // 404 and 410 are different dead ends and the card says so: one may be
        // fixed by a plan change, the other never will.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(410).set_body_string("end of life"))
            .mount(&server)
            .await;

        let outcome = probe_model(&server.uri(), Some("nvapi-k"), "meta/retired").await;
        assert_eq!(outcome, ModelProbe::Retired);
        assert_eq!(verdict_of(&outcome).0, NvidiaProbeVerdict::Retired);
    }

    #[test]
    fn each_wire_provider_reads_its_own_endpoint_slot() {
        use crate::models::setup::HttpEndpoints;

        // The bug this pins (KT-337): the NVIDIA slot was declared and read but
        // never written, so a configured NVIDIA endpoint was ignored and the
        // public default silently won. A provider must never see another's.
        let endpoints = HttpEndpoints {
            lite_llm: Some("http://proxy.internal:4000".into()),
            nvidia: Some("https://nim.internal:8000".into()),
        };
        assert_eq!(
            endpoints.for_agent(&AgentType::Nvidia),
            Some("https://nim.internal:8000")
        );
        assert_eq!(
            endpoints.for_agent(&AgentType::LiteLlm),
            Some("http://proxy.internal:4000")
        );

        // A non-wire agent gets NOTHING rather than LiteLLM's proxy. Before the
        // match was made exhaustive, the catch-all handed the proxy url to every
        // agent — harmless only because Ollama ignores the value. The exhaustive
        // match is what stops the NEXT wire provider from inheriting it.
        assert_eq!(endpoints.for_agent(&AgentType::Ollama), None);
        assert_eq!(endpoints.for_agent(&AgentType::ClaudeCode), None);

        // And an unset slot stays unset rather than borrowing the other one,
        // so the fallback is the provider's own default.
        let only_proxy = HttpEndpoints {
            lite_llm: Some("http://proxy.internal:4000".into()),
            nvidia: None,
        };
        assert_eq!(only_proxy.for_agent(&AgentType::Nvidia), None);
        assert_eq!(
            resolve_base_url(only_proxy.for_agent(&AgentType::Nvidia)),
            DEFAULT_BASE_URL,
            "with no NVIDIA endpoint saved, the hosted default answers — never the proxy"
        );
    }

    #[test]
    fn nvidia_tiers_are_selectable_independently_of_litellm() {
        // Same three tiers, separate storage: assigning a NVIDIA model must not
        // move LiteLLM's, which is what a single shared tier block would do.
        let mut tiers = crate::models::ModelTiersConfig::default();
        tiers.lite_llm.default = Some("corp-proxy-model".into());
        tiers.nvidia.economy = Some("meta/llama-3.1-8b-instruct".into());
        tiers.nvidia.default = Some("meta/llama-3.3-70b-instruct".into());
        tiers.nvidia.reasoning = Some("deepseek-ai/deepseek-r1".into());

        assert_eq!(tiers.lite_llm.default.as_deref(), Some("corp-proxy-model"));
        assert_eq!(tiers.lite_llm.economy, None);
        assert_eq!(
            tiers.nvidia.reasoning.as_deref(),
            Some("deepseek-ai/deepseek-r1")
        );
    }
}
