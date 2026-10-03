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
//! exposes whether a credential is present, never its value. The value only
//! crosses the wire after an explicit authenticated reveal action.

use crate::core::config;
use crate::db::{external_api_connections as store, model_catalog as catalog_store};
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
    /// Media generation slots. Optional: a provider with no media catalogue
    /// simply leaves them empty.
    #[serde(default)]
    pub image_model: Option<String>,
    #[serde(default)]
    pub video_model: Option<String>,
    /// Override for providers serving media from another host. Empty derives
    /// it from `endpoint`.
    #[serde(default)]
    pub media_endpoint: Option<String>,
    /// Keep a tier model even though a call to it was just refused. Without
    /// it, such a save is refused with `unreachable_model` (KT-957).
    #[serde(default)]
    pub confirm_unreachable_models: bool,
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
    /// Optional exact models to verify after loading the catalogue. NVIDIA's
    /// public catalogue is not an entitlement list, so its first entry must
    /// never be used as an implicit connectivity probe.
    #[serde(default)]
    pub models: Vec<String>,
    /// The model chosen for each tier (`economy` | `default` | `reasoning`).
    /// When present for a LiteLLM connection, each one gets its own minimal
    /// call and the answer is reported per tier in `tier_checks` (KT-941);
    /// `models` alone keeps the older all-or-nothing verdict.
    #[serde(default)]
    pub tier_models: Vec<TierModelRequest>,
    /// Caller-chosen id under which the model sweep reports how far it got,
    /// read back from `GET /api/external-api/connections/test/progress/{id}`.
    #[serde(default)]
    pub progress_id: Option<String>,
}

/// How far a connection test's model sweep has got.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TestProgress {
    pub done: usize,
    pub total: usize,
}

/// Sweeps in flight, by caller id. A sweep is a minute at most; entries older
/// than ten are dropped on the next write.
type ProgressMap = std::collections::HashMap<String, (TestProgress, std::time::Instant)>;
static TEST_PROGRESS: std::sync::LazyLock<std::sync::Mutex<ProgressMap>> =
    std::sync::LazyLock::new(Default::default);

fn valid_progress_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn set_test_progress(id: Option<&str>, progress: TestProgress) {
    let Some(id) = id.filter(|id| valid_progress_id(id)) else {
        return;
    };
    if let Ok(mut map) = TEST_PROGRESS.lock() {
        map.retain(|_, (_, at)| at.elapsed() < std::time::Duration::from_secs(600));
        map.insert(id.to_string(), (progress, std::time::Instant::now()));
    }
}

/// GET /api/external-api/connections/test/progress/{id}
pub async fn test_progress(Path(id): Path<String>) -> Json<ApiResponse<TestProgress>> {
    let progress = TEST_PROGRESS
        .lock()
        .ok()
        .and_then(|map| map.get(&id).map(|(progress, _)| *progress))
        .unwrap_or_default();
    Json(ApiResponse::ok(progress))
}

#[derive(Debug, Clone, Deserialize)]
pub struct TierModelRequest {
    pub tier: String,
    pub model: String,
}

/// A validated tier → model pair of a connection test.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TierModel {
    tier: &'static str,
    model: String,
}

/// What one tier's model answered to a real one-token call (KT-941). Carries a
/// status and a generic hint only — never the upstream body, which can echo
/// account metadata, and never the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TierCheck {
    pub tier: String,
    pub model: String,
    pub ok: bool,
    /// `ok` | `not_found` | `access_denied` | `http_error` | `timeout` |
    /// `transport_error`.
    pub status: String,
    pub http_status: Option<u16>,
    pub hint: Option<String>,
}

/// What one listed model answered when its LiteLLM connection was tested
/// (KT-957). Same statuses as [`TierCheck`], never the upstream body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCheck {
    pub model: String,
    pub ok: bool,
    pub status: String,
    pub http_status: Option<u16>,
}

#[derive(Debug, Default, Serialize)]
pub struct TestConnectionResponse {
    pub ok: bool,
    pub status: String,
    /// Backward-compatible chat model ids consumed by tier selectors.
    pub models: Vec<String>,
    /// Capability-bearing union of every catalogue endpoint reached during
    /// the probe. Media selectors filter this list instead of treating chat
    /// models (or a previously saved free-text value) as media-capable.
    pub catalog: Vec<TestConnectionModel>,
    /// Whether the image/video modality is provable from catalog evidence —
    /// either `architecture.output_modalities` on the chat catalogue, or a
    /// dedicated capability endpoint that answered. NVIDIA's `/v1/models`
    /// carries neither, so both stay false: the picker cannot rule anything
    /// in or out and must say so instead of silently filtering to nothing.
    pub image_capability_known: bool,
    pub video_capability_known: bool,
    pub hint: Option<String>,
    /// One entry per tier the caller asked to verify; empty otherwise.
    pub tier_checks: Vec<TierCheck>,
    /// LiteLLM only: one entry per listed chat model called during the test.
    pub model_checks: Vec<ModelCheck>,
    /// LiteLLM only: the route that said what each model is for
    /// (`model_info` | `model_group_info`); `None` when the proxy said nothing.
    pub capability_source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestConnectionModel {
    pub id: String,
    pub display_name: String,
    pub capabilities: Vec<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn validate_credential_format(
    preset: ExternalApiConnectionPreset,
    api_key: Option<&str>,
) -> Result<(), &'static str> {
    let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(());
    };
    if preset == ExternalApiConnectionPreset::OpenRouter && !key.starts_with("sk-or-v1-") {
        return Err("The OpenRouter API key must include its sk-or-v1- prefix");
    }
    Ok(())
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
/// 2. validate OpenRouter through its non-billable current-key endpoint, or
///    confirm generic credentials with a minimal authenticated model call.
///
/// Phase 2 exists because `/v1/models` is public on several providers
/// (LiteLLM, some NVIDIA/Groq deployments): a catalogue read alone lets an
/// invalid key through. The chat call is rejected with 401/403 when the key is
/// wrong, so an invalid key can no longer pass. Neither phase surfaces an
/// upstream body or the submitted key.
async fn probe_models(
    endpoint: &str,
    api_key: Option<&str>,
    origin_preset: Option<ExternalApiConnectionPreset>,
    requested_models: &[String],
    tier_models: &[TierModel],
) -> TestConnectionResponse {
    let is_open_router = origin_preset == Some(ExternalApiConnectionPreset::OpenRouter);
    if is_open_router {
        let Some(key) = api_key.filter(|key| !key.trim().is_empty()) else {
            return TestConnectionResponse {
                ok: false,
                status: "credential_required".into(),
                models: vec![],
                catalog: vec![],
                hint: Some("Enter the OpenRouter API key before testing this connection.".into()),
                ..Default::default()
            };
        };
        if let Some(failure) = probe_openrouter_credential(endpoint, key).await {
            return failure;
        }
    }

    let mut catalogue = fetch_catalogue(endpoint, api_key).await;
    if catalogue.ok && is_open_router {
        let (images, videos) = tokio::join!(
            fetch_capability_catalogue(endpoint, api_key, "/v1/images/models", "image"),
            fetch_capability_catalogue(endpoint, api_key, "/v1/videos/models", "video"),
        );
        let (image_models, image_reached) = images;
        let (video_models, video_reached) = videos;
        merge_catalog(&mut catalogue.catalog, image_models);
        merge_catalog(&mut catalogue.catalog, video_models);
        // A capability endpoint that answered is authoritative on its own,
        // even if it listed zero models this time (state C: proven, not
        // merely unfiltered) — independent of whatever the chat catalogue's
        // `architecture` field said.
        catalogue.image_capability_known |= image_reached;
        catalogue.video_capability_known |= video_reached;
    }
    if catalogue.ok {
        let is_nvidia = origin_preset == Some(ExternalApiConnectionPreset::Nvidia);
        if (is_nvidia || is_open_router) && api_key.filter(|key| !key.trim().is_empty()).is_none() {
            catalogue.ok = false;
            catalogue.status = "credential_required".into();
            catalogue.hint = Some(
                "The model catalogue is available, but an API key is required to run a model."
                    .into(),
            );
            return catalogue;
        }
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            if (is_nvidia || is_open_router) && requested_models.is_empty() {
                catalogue.hint = Some(
                    "Model catalogue loaded. Select the tier models, then test again to verify access for this account."
                        .into(),
                );
                return catalogue;
            }
            if is_nvidia || is_open_router {
                for model in requested_models {
                    if let Some(auth_failure) = probe_auth(
                        endpoint,
                        key,
                        model,
                        is_nvidia.then_some(catalogue.models.as_slice()),
                    )
                    .await
                    {
                        return auth_failure;
                    }
                }
                catalogue.hint = Some(if is_nvidia {
                    format!(
                        "{} selected NVIDIA model(s) answered successfully and are accessible to this account.",
                        requested_models.len()
                    )
                } else {
                    format!(
                        "{} selected OpenRouter model(s) answered successfully.",
                        requested_models.len()
                    )
                });
            } else if origin_preset == Some(ExternalApiConnectionPreset::LiteLlm)
                && !tier_models.is_empty()
            {
                // A LiteLLM proxy lists aliases its upstream cannot serve, so
                // one refused model must not fail the connection or empty the
                // pickers: report each tier and let the operator re-choose.
                return probe_tier_models(endpoint, key, catalogue, tier_models).await;
            } else if !requested_models.is_empty() {
                for model in requested_models {
                    if let Some(auth_failure) = probe_auth(endpoint, key, model, None).await {
                        return auth_failure;
                    }
                }
                catalogue.hint = Some(format!(
                    "{} configured model(s) answered successfully.",
                    requested_models.len()
                ));
            } else if !catalogue.catalog.is_empty() {
                // Nothing configured yet — the common case when an operator
                // tests a connection before picking tiers. The catalogue alone
                // cannot answer the question: a proxy may serve `/v1/models`
                // publicly while chat needs the key, so stopping here reported
                // an invalid credential as a working connection.
                //
                // But probing whatever the proxy listed first sent a chat
                // completion to an embedding or rerank deployment, which
                // answers 400, and a healthy connection was reported broken.
                //
                // Both are avoided by reading the answer rather than the
                // catalogue: 401/403 is a verdict on the CREDENTIAL and ends
                // the probe, while 400/404 is a verdict on the MODEL and says
                // nothing about the key, so the next entry is tried. A
                // catalogue that declares its modalities usually puts a chat
                // model first anyway; this holds when it declares nothing,
                // which is the common case for an OpenAI-compatible proxy.
                let mut inconclusive: Option<TestConnectionResponse> = None;
                let mut answered: Option<String> = None;
                for entry in catalogue.catalog.iter().take(MAX_FALLBACK_PROBES) {
                    match probe_auth(endpoint, key, &entry.id, None).await {
                        None => {
                            answered = Some(entry.id.clone());
                            break;
                        }
                        // The model is wrong, not the key. Keep the last one to
                        // report if nothing in the catalogue ever answers.
                        Some(failure) if failure.status == "http_error" => {
                            inconclusive = Some(failure);
                        }
                        Some(failure) => return failure,
                    }
                }
                match answered {
                    Some(model) => {
                        catalogue.hint = Some(format!(
                            "The credential works: {model} answered. Select the tier models to verify the ones you will use."
                        ));
                    }
                    // Every entry tried refused the request shape. That is not
                    // an authentication verdict and must not be dressed as one.
                    // Returned as-is: a failed connection test hands the tier
                    // selectors no models, so they cannot offer a choice the
                    // probe was unable to verify.
                    None => {
                        if let Some(failure) = inconclusive {
                            return failure;
                        }
                    }
                }
            }
        }
    }
    catalogue
}

/// How many catalogue entries the credential check will try before giving up.
///
/// Each is a real round trip, and a connection test the operator is waiting on
/// must stay bounded. Four is enough to walk past a run of embedding or rerank
/// deployments without turning the test into a catalogue sweep.
const MAX_FALLBACK_PROBES: usize = 4;

const BILLING_ERROR_HINT: &str = "The provider requires payment or additional credits (HTTP 402). Check your API account balance and billing, then test again.";

/// Hint shown when a probed model answers a non-2xx status.
///
/// The generic arm names the model on purpose: a bare status told the operator
/// that "the connection" failed, when what failed was one model — and before
/// the caller was fixed, a model they had never configured.
fn http_error_hint(status: u16, model: &str, nvidia_catalogue: bool) -> String {
    match (status, nvidia_catalogue) {
        (404, true) => format!(
            "The NVIDIA model {model} is listed publicly but is not accessible to this account. Choose another model or check the account's NVIDIA API permissions."
        ),
        (410, true) => format!(
            "The NVIDIA model {model} has been retired. Choose another model from the catalogue."
        ),
        _ => format!(
            "The endpoint returned HTTP {status} for model {model} while validating the connection."
        ),
    }
}

/// OpenRouter's model catalogue is public, so a successful `/models` response
/// says nothing about the submitted credential. Its dedicated current-key
/// endpoint is non-billable and validates the Bearer token before the UI
/// unlocks model selection. The upstream response body is deliberately never
/// surfaced because it contains account and usage metadata.
async fn probe_openrouter_credential(
    endpoint: &str,
    api_key: &str,
) -> Option<TestConnectionResponse> {
    let request = probe_client()
        .get(format!("{endpoint}/v1/key"))
        .bearer_auth(api_key);
    match request.send().await {
        Ok(response) if response.status().is_success() => None,
        Ok(response) if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED => {
            Some(TestConnectionResponse {
                status: "billing_error".into(),
                hint: Some(BILLING_ERROR_HINT.into()),
                ..Default::default()
            })
        }
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => {
            let hint = if !api_key.starts_with("sk-or-v1-")
                && api_key.len() == 64
                && api_key.chars().all(|ch| ch.is_ascii_hexdigit())
            {
                "The saved OpenRouter key is incomplete. Replace it with the full key, including its sk-or-v1- prefix."
            } else {
                "OpenRouter rejected this API key. Replace it with an active key and check its permissions."
            };
            Some(TestConnectionResponse {
                ok: false,
                status: "auth_error".into(),
                models: vec![],
                catalog: vec![],
                hint: Some(hint.into()),
                ..Default::default()
            })
        }
        Ok(response) => Some(TestConnectionResponse {
            ok: false,
            status: "http_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(format!(
                "OpenRouter returned HTTP {} while validating the API key.",
                response.status().as_u16()
            )),
            ..Default::default()
        }),
        Err(error) if error.is_timeout() => Some(TestConnectionResponse {
            ok: false,
            status: "timeout".into(),
            models: vec![],
            catalog: vec![],
            hint: Some("OpenRouter did not answer the API key validation in time.".into()),
            ..Default::default()
        }),
        Err(_) => Some(TestConnectionResponse {
            ok: false,
            status: "transport_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some("Kronn could not reach OpenRouter to validate the API key.".into()),
            ..Default::default()
        }),
    }
}

/// Minimal authenticated invocation confirming the credential is accepted. A
/// `max_tokens: 1` request is the smallest billable/no-output probe compatible
/// with the OpenAI chat contract. A 2xx response confirms the credential;
/// 401/403 are classified as authentication errors, while every other HTTP
/// status is a generic probe failure because it does not prove that the
/// connection is usable. A 401/403 keeps the `auth_error` status — a public
/// catalogue does not prove the credential either way — but its message names
/// BOTH causes, a rejected key and a model out of reach, instead of blaming the
/// key alone. `None` = the model answered.
async fn probe_auth(
    endpoint: &str,
    api_key: &str,
    model: &str,
    retained_models: Option<&[String]>,
) -> Option<TestConnectionResponse> {
    failure_response(
        model,
        &chat_probe(endpoint, api_key, model).await,
        retained_models,
    )
}

/// The raw outcome of one minimal chat call, before it becomes a verdict.
/// Bodies are kept only so a refusal can be classified (a LiteLLM allow-list
/// says so in its body); they are never surfaced.
#[derive(Debug)]
enum ChatProbe {
    Answered,
    Billing,
    /// 401/403.
    Refused {
        status: u16,
        body: String,
    },
    /// Any other non-2xx.
    Http {
        status: u16,
        body: String,
    },
    Timeout,
    Transport,
}

/// For calls whose answer is a verdict on a model. A proxy can take several
/// seconds to refuse one (a tag refusal measured at 9 s), which the 6 s
/// connection probe reported as a timeout and so let through.
fn model_probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default()
}

async fn chat_probe(endpoint: &str, api_key: &str, model: &str) -> ChatProbe {
    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "temperature": 0,
        "stream": false,
    });
    let request = model_probe_client()
        .post(format!("{endpoint}/v1/chat/completions"))
        .bearer_auth(api_key)
        .json(&body);
    match request.send().await {
        Ok(response) if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED => {
            ChatProbe::Billing
        }
        Ok(response) if response.status().is_success() => ChatProbe::Answered,
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if matches!(status, 401 | 403) {
                ChatProbe::Refused { status, body }
            } else {
                ChatProbe::Http { status, body }
            }
        }
        Err(error) if error.is_timeout() => ChatProbe::Timeout,
        Err(_) => ChatProbe::Transport,
    }
}

/// The connection-level failure a probe stands for; `None` = the model
/// answered. Billing failures must stop the fallback before a later model error
/// can hide the reason this account cannot generate a response. A 401/403 has
/// TWO possible causes and the message must not pick one: the key may be
/// rejected, or the key may be valid while this model is out of reach for the
/// account, or simply not served by this endpoint. Blaming the key alone sent
/// users hunting a credential that was fine — observed on an image-generation
/// model, which lives on another endpoint. The status stays `auth_error`
/// because a public catalogue does not prove the credential either.
fn failure_response(
    model: &str,
    probe: &ChatProbe,
    retained_models: Option<&[String]>,
) -> Option<TestConnectionResponse> {
    match probe {
        ChatProbe::Answered => None,
        ChatProbe::Billing => Some(TestConnectionResponse {
            status: "billing_error".into(),
            hint: Some(BILLING_ERROR_HINT.into()),
            ..Default::default()
        }),
        ChatProbe::Refused { .. } => Some(TestConnectionResponse {
            ok: false,
            status: "auth_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(format!(
                "The endpoint refused '{model}'. Either the API key is rejected, or the key is valid but this model is not available to this account on this endpoint — image and video models are often served elsewhere. Check both before replacing the key."
            )),
            ..Default::default()
        }),
        ChatProbe::Http { status, .. } => {
            let models = retained_models
                .map(|items| items.to_vec())
                .unwrap_or_default();
            let hint = http_error_hint(*status, model, retained_models.is_some());
            Some(TestConnectionResponse {
                ok: false,
                status: "http_error".into(),
                models,
                catalog: vec![],
                hint: Some(hint),
                ..Default::default()
            })
        }
        ChatProbe::Timeout => Some(TestConnectionResponse {
            ok: false,
            status: "timeout".into(),
            models: retained_models
                .map(|items| items.to_vec())
                .unwrap_or_default(),
            catalog: vec![],
            hint: Some(if retained_models.is_some() {
                format!(
                    "The NVIDIA model {model} did not answer in time. Try it again or choose another model."
                )
            } else {
                "The endpoint did not respond in time. Check its URL and availability.".into()
            }),
            ..Default::default()
        }),
        ChatProbe::Transport => Some(TestConnectionResponse {
            ok: false,
            status: "transport_error".into(),
            models: retained_models
                .map(|items| items.to_vec())
                .unwrap_or_default(),
            catalog: vec![],
            hint: Some(if retained_models.is_some() {
                format!(
                    "Kronn could not reach the NVIDIA model {model}. Try another model or check NVIDIA availability."
                )
            } else {
                "Kronn could not reach this endpoint. Check the URL and network access.".into()
            }),
            ..Default::default()
        }),
    }
}

/// Probe the model of every tier with a real one-token call and report each
/// answer (KT-941). `catalogue` is the already-loaded `/v1/models` response and
/// stays the result's base: a model that does not answer is a verdict on that
/// tier, not on the connection, so the pickers keep their catalogue and the
/// operator can pick another model.
///
/// The key is judged by what the proxy did, not by one refusal: a proxy checks
/// the key before it looks at the model, so ANY answer other than a bare
/// 401/403 — a success, a 404, a 500, a refusal that names the model — proves
/// the key was accepted, and the remaining refusals are then that model's
/// allow-list. Only when nothing shows the key was accepted is it reported as
/// a credential or reachability failure, exactly as before.
async fn probe_tier_models(
    endpoint: &str,
    api_key: &str,
    catalogue: TestConnectionResponse,
    tier_models: &[TierModel],
) -> TestConnectionResponse {
    let mut distinct: Vec<&str> = Vec::new();
    for entry in tier_models {
        if !distinct.contains(&entry.model.as_str()) {
            distinct.push(&entry.model);
        }
    }
    let answers = futures::future::join_all(
        distinct
            .iter()
            .map(|model| chat_probe(endpoint, api_key, model)),
    )
    .await;
    let probes: Vec<(&str, ChatProbe)> = distinct.into_iter().zip(answers).collect();
    assemble_tier_report(catalogue, tier_models, &probes)
}

/// The verdict half of [`probe_tier_models`], free of any network call:
/// `probes` holds one answer per distinct model of `tier_models`.
fn assemble_tier_report(
    mut catalogue: TestConnectionResponse,
    tier_models: &[TierModel],
    probes: &[(&str, ChatProbe)],
) -> TestConnectionResponse {
    if let Some((model, probe)) = probes
        .iter()
        .find(|(_, probe)| matches!(probe, ChatProbe::Billing))
    {
        return failure_response(model, probe, None).expect("billing is a failure");
    }
    if !key_accepted(probes) {
        let (model, probe) = &probes[0];
        return failure_response(model, probe, None)
            .expect("a probe that did not answer is a failure");
    }

    let checks: Vec<TierCheck> = tier_models
        .iter()
        .map(|entry| {
            let (_, probe) = probes
                .iter()
                .find(|(model, _)| *model == entry.model)
                .expect("every tier model was probed");
            tier_check(entry, probe)
        })
        .collect();
    let failing: Vec<String> = checks
        .iter()
        .filter(|check| !check.ok)
        .map(|check| {
            format!(
                "The {} model {} does not answer: {}",
                check.tier,
                check.model,
                check.hint.as_deref().unwrap_or(&check.status)
            )
        })
        .collect();
    if failing.is_empty() {
        catalogue.hint = Some(format!(
            "{} configured model(s) answered successfully.",
            probes.len()
        ));
    } else {
        catalogue.status = "model_error".into();
        catalogue.hint = Some(failing.join(" "));
    }
    catalogue.tier_checks = checks;
    catalogue
}

/// Write what the tier calls proved into the catalogue: a model that answered
/// loses a previous not-found / access-denied flag, one that was refused gets
/// it, with the reason a picker shows. A timeout or an unclassified error
/// proves nothing lasting about the model and leaves it as it was.
fn record_tier_verdicts(
    conn: &rusqlite::Connection,
    runtime_target_id: &str,
    checks: &[TierCheck],
) -> anyhow::Result<()> {
    record_model_verdicts(
        conn,
        runtime_target_id,
        checks.iter().map(|check| {
            (
                check.model.as_str(),
                check.status.as_str(),
                check.http_status,
            )
        }),
    )
}

/// [`record_tier_verdicts`] for any `(model, status, http_status)` verdicts.
fn record_model_verdicts<'a>(
    conn: &rusqlite::Connection,
    runtime_target_id: &str,
    verdicts: impl IntoIterator<Item = (&'a str, &'a str, Option<u16>)>,
) -> anyhow::Result<()> {
    for (model, status, http_status) in verdicts {
        let reason = match status {
            "ok" => {
                catalog_store::clear_model_failure(conn, runtime_target_id, model)?;
                continue;
            }
            "not_found" => ModelUnavailableReason::NotFound,
            "access_denied" => ModelUnavailableReason::AccessDenied,
            _ => continue,
        };
        catalog_store::mark_unavailable(
            conn,
            runtime_target_id,
            model,
            reason,
            Some(&crate::api::lite_llm::model_failure_detail(
                reason,
                http_status.unwrap_or_default(),
            )),
        )?;
    }
    Ok(())
}

/// How many listed models a LiteLLM test calls at once, and for how long in
/// total; a model not reached in time keeps whatever was known about it.
const SWEEP_PARALLELISM: usize = 12;
const SWEEP_BUDGET: std::time::Duration = std::time::Duration::from_secs(75);

/// One minimal call to every chat model a LiteLLM proxy lists (KT-957): it
/// lists models it cannot serve, and only a call tells them apart. Runs once
/// the key is known to be accepted, so a refusal is the model's. Models the
/// tier checks already called reuse that answer.
async fn sweep_listed_models(
    endpoint: &str,
    api_key: &str,
    catalog: &[TestConnectionModel],
    tier_checks: &[TierCheck],
    progress_id: Option<&str>,
) -> Vec<ModelCheck> {
    use futures::StreamExt;

    let mut checks: Vec<ModelCheck> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for entry in catalog {
        let chat = entry
            .capabilities
            .iter()
            .any(|capability| capability == "chat");
        if !chat
            || checks.iter().any(|check| check.model == entry.id)
            || pending.contains(&entry.id)
        {
            continue;
        }
        match tier_checks.iter().find(|check| check.model == entry.id) {
            Some(check) => checks.push(model_check(&entry.id, check)),
            None => pending.push(entry.id.clone()),
        }
    }
    let total = checks.len() + pending.len();
    let report = |done| set_test_progress(progress_id, TestProgress { done, total });
    report(checks.len());
    let deadline = tokio::time::Instant::now() + SWEEP_BUDGET;
    let mut answers = futures::stream::iter(pending)
        .map(|model| async move {
            let probe = chat_probe(endpoint, api_key, &model).await;
            let entry = TierModel {
                tier: "catalog",
                model,
            };
            let check = tier_check(&entry, &probe);
            model_check(&entry.model, &check)
        })
        .buffer_unordered(SWEEP_PARALLELISM);
    while let Ok(Some(check)) = tokio::time::timeout_at(deadline, answers.next()).await {
        checks.push(check);
        report(checks.len());
    }
    checks
}

/// What a LiteLLM proxy says each listed model is for, from `model_info.mode`
/// (KT-957): chat, image or video generation, embedding… `/model/info` first,
/// `/model_group/info` when a non-admin key is refused the former. `None` when
/// neither says anything, and the catalogue then stays as listed.
async fn lite_llm_model_modes(
    endpoint: &str,
    api_key: Option<&str>,
) -> Option<(&'static str, std::collections::HashMap<String, Vec<String>>)> {
    for (source, path) in [
        ("model_info", "/model/info"),
        ("model_group_info", "/model_group/info"),
    ] {
        let mut request = probe_client().get(format!("{endpoint}{path}"));
        if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
            request = request.bearer_auth(key);
        }
        let Ok(response) = request.send().await else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(body) = response.json::<serde_json::Value>().await else {
            continue;
        };
        let modes = model_modes_from_body(&body);
        if !modes.is_empty() {
            return Some((source, modes));
        }
    }
    None
}

/// `/model/info` (`model_name` + `model_info`) and `/model_group/info`
/// (`model_group` + flat fields) answers, as capabilities per model id.
fn model_modes_from_body(
    body: &serde_json::Value,
) -> std::collections::HashMap<String, Vec<String>> {
    let mut modes: std::collections::HashMap<String, Vec<String>> = Default::default();
    for item in body["data"].as_array().into_iter().flatten() {
        let (Some(id), info) = (
            item["model_name"]
                .as_str()
                .or_else(|| item["model_group"].as_str()),
            if item["model_info"].is_object() {
                &item["model_info"]
            } else {
                item
            },
        ) else {
            continue;
        };
        let Some(mode) = info["mode"].as_str() else {
            continue;
        };
        let capability = match mode {
            "chat" | "completion" | "responses" => "chat",
            "image_generation" | "image_edit" => "image",
            "video_generation" => "video",
            other => other,
        };
        let entry = modes.entry(id.to_string()).or_default();
        if !entry.iter().any(|known| known == capability) {
            entry.push(capability.to_string());
        }
        if info["supports_vision"] == true && !entry.iter().any(|known| known == "vision") {
            entry.push("vision".into());
        }
    }
    modes
}

/// Replace the listed models' guessed `chat` with what the proxy declared.
fn apply_model_modes(
    response: &mut TestConnectionResponse,
    modes: &std::collections::HashMap<String, Vec<String>>,
) {
    for entry in &mut response.catalog {
        if let Some(capabilities) = modes.get(&entry.id) {
            entry.capabilities = capabilities.clone();
        }
    }
    response.image_capability_known = true;
    response.video_capability_known = true;
}

fn model_check(model: &str, check: &TierCheck) -> ModelCheck {
    ModelCheck {
        model: model.to_string(),
        ok: check.ok,
        status: check.status.clone(),
        http_status: check.http_status,
    }
}

/// Whether any answer shows the proxy accepted the key, which makes the
/// remaining refusals verdicts on their models rather than on the key.
fn key_accepted(probes: &[(&str, ChatProbe)]) -> bool {
    probes.iter().any(|(_, probe)| match probe {
        ChatProbe::Answered | ChatProbe::Http { .. } => true,
        ChatProbe::Refused { status, body } => {
            crate::api::lite_llm::classify_model_failure(*status, body).is_some()
        }
        ChatProbe::Billing | ChatProbe::Timeout | ChatProbe::Transport => false,
    })
}

/// The tier models a save assigns: all of them on creation, the changed ones
/// on edit. An unchanged model was already accepted and is not re-judged.
fn assigned_tier_models(
    previous: Option<&ExternalApiConnection>,
    next: &ExternalApiConnection,
) -> Vec<TierModel> {
    let tiers = |connection: &ExternalApiConnection| {
        [
            ("economy", connection.economy_model.clone()),
            ("default", connection.default_model.clone()),
            ("reasoning", connection.reasoning_model.clone()),
        ]
    };
    let before = previous.map(tiers);
    tiers(next)
        .into_iter()
        .enumerate()
        .filter_map(|(index, (tier, model))| {
            let model = model?;
            let unchanged = before
                .as_ref()
                .is_some_and(|before| before[index].1.as_deref() == Some(model.as_str()));
            (!unchanged).then_some(TierModel { tier, model })
        })
        .collect()
}

/// One minimal call per model a LiteLLM save assigns (KT-957). Empty when
/// nothing can be judged: no key, or no answer shows the key was accepted,
/// since a key problem is not the model's.
async fn probe_assigned_tier_models(
    endpoint: &str,
    api_key: Option<&str>,
    tier_models: &[TierModel],
) -> Vec<TierCheck> {
    let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) else {
        return Vec::new();
    };
    let mut distinct: Vec<&str> = Vec::new();
    for entry in tier_models {
        if !distinct.contains(&entry.model.as_str()) {
            distinct.push(&entry.model);
        }
    }
    let answers = futures::future::join_all(
        distinct
            .iter()
            .map(|model| chat_probe(endpoint, api_key, model)),
    )
    .await;
    let probes: Vec<(&str, ChatProbe)> = distinct.into_iter().zip(answers).collect();
    assigned_tier_verdicts(tier_models, &probes)
}

/// The network-free half of [`probe_assigned_tier_models`].
fn assigned_tier_verdicts(
    tier_models: &[TierModel],
    probes: &[(&str, ChatProbe)],
) -> Vec<TierCheck> {
    if !key_accepted(probes) {
        return Vec::new();
    }
    tier_models
        .iter()
        .filter_map(|entry| {
            probes
                .iter()
                .find(|(model, _)| *model == entry.model)
                .map(|(_, probe)| tier_check(entry, probe))
        })
        .collect()
}

/// The assigned models the catalogue already knows the proxy refuses, when the
/// save's own call proved nothing either way (no key, a timeout).
fn remembered_refusals(
    known: &[CatalogModelEntry],
    assigned: &[TierModel],
    checks: &[TierCheck],
) -> Vec<TierCheck> {
    assigned
        .iter()
        .filter(|entry| {
            !checks.iter().any(|check| {
                check.model == entry.model
                    && matches!(check.status.as_str(), "ok" | "not_found" | "access_denied")
            })
        })
        .filter_map(|entry| {
            let known = known.iter().find(|known| known.model_id == entry.model)?;
            let status = match known.unavailable_reason? {
                ModelUnavailableReason::NotFound => "not_found",
                ModelUnavailableReason::AccessDenied => "access_denied",
                _ => return None,
            };
            Some(TierCheck {
                tier: entry.tier.to_string(),
                model: entry.model.clone(),
                ok: false,
                status: status.to_string(),
                http_status: None,
                hint: known.unavailable_detail.clone(),
            })
        })
        .collect()
}

/// The models a save must not keep without an explicit confirmation.
fn refused_tier_checks(checks: &[TierCheck]) -> Vec<&TierCheck> {
    checks
        .iter()
        .filter(|check| matches!(check.status.as_str(), "not_found" | "access_denied"))
        .collect()
}

/// The refusal a save gets when a model it assigns was just refused and the
/// operator has not confirmed keeping it.
fn unconfirmed_refusal<T: Serialize>(
    checks: &[TierCheck],
    confirmed: bool,
) -> Option<ApiResponse<T>> {
    let refused = refused_tier_checks(checks);
    (!confirmed && !refused.is_empty()).then(|| {
        ApiResponse::err_coded(
            ApiErrorCode::UnreachableModel,
            unreachable_models_message(&refused),
        )
    })
}

fn unreachable_models_message(refused: &[&TierCheck]) -> String {
    let models = refused
        .iter()
        .map(|check| match check.http_status {
            Some(status) => format!("{} ({} tier, HTTP {status})", check.model, check.tier),
            None => format!(
                "{} ({} tier, refused when last called)",
                check.model, check.tier
            ),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "The proxy lists but refuses to serve {models}: not found upstream, or not allowed for this key or project. Choose another model, or confirm to keep it anyway."
    )
}

fn tier_check(entry: &TierModel, probe: &ChatProbe) -> TierCheck {
    let refused = |status: u16| {
        (
            "access_denied",
            Some(status),
            Some(format!(
                "HTTP {status}: the proxy refuses this model for this key or project. Choose another model for this tier."
            )),
        )
    };
    let (status, http_status, hint): (&str, Option<u16>, Option<String>) = match probe {
        ChatProbe::Answered => ("ok", None, None),
        // Reached only once another answer proved the key accepted, so this
        // refusal is the model's own allow-list.
        ChatProbe::Refused { status, .. } => refused(*status),
        ChatProbe::Http { status, body } => {
            match crate::api::lite_llm::classify_model_failure(*status, body) {
                Some(ModelUnavailableReason::NotFound) => (
                    "not_found",
                    Some(*status),
                    Some(format!(
                        "HTTP {status}: the proxy lists this model but its upstream deployment was not found, or access to it is denied. Choose another model for this tier."
                    )),
                ),
                Some(_) => refused(*status),
                None => (
                    "http_error",
                    Some(*status),
                    Some(format!(
                        "HTTP {status}: the model answered with an error — it may not be a chat model, or its upstream is failing. Choose another model for this tier."
                    )),
                ),
            }
        }
        ChatProbe::Timeout => (
            "timeout",
            None,
            Some("no answer in time. Test again or choose another model.".into()),
        ),
        ChatProbe::Billing | ChatProbe::Transport => (
            "transport_error",
            None,
            Some("Kronn could not reach the proxy for this model.".into()),
        ),
    };
    TierCheck {
        tier: entry.tier.to_string(),
        model: entry.model.clone(),
        ok: status == "ok",
        status: status.to_string(),
        http_status,
        hint,
    }
}

async fn fetch_catalogue(endpoint: &str, api_key: Option<&str>) -> TestConnectionResponse {
    let mut request = probe_client().get(format!("{endpoint}/v1/models"));
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    match request.send().await {
        Ok(response) if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED => {
            TestConnectionResponse {
                status: "billing_error".into(),
                hint: Some(BILLING_ERROR_HINT.into()),
                ..Default::default()
            }
        }
        Ok(response) if response.status().is_success() => {
            match response.json::<serde_json::Value>().await {
                Ok(body) if model_ids_from_body(&body).is_some() => {
                    let models = model_ids_from_body(&body).expect("checked above");
                    let catalog = catalog_models_from_body(&body, "chat").unwrap_or_default();
                    // Catalog evidence, not the provider's name: a chat catalogue
                    // that never declares `architecture.output_modalities` on any
                    // model (NVIDIA's `/v1/models` today) cannot rule image/video
                    // in or out for ANY of its entries, known or not.
                    let modality_declared = catalog_declares_modality(&body);
                    TestConnectionResponse {
                    ok: true,
                    status: "success".into(),
                    hint: models.is_empty().then(|| "The endpoint responded but returned no usable models. Check this account or endpoint.".into()),
                    models,
                    catalog,
                    image_capability_known: modality_declared,
                    video_capability_known: modality_declared,
                    tier_checks: Vec::new(),
                    model_checks: Vec::new(),
                    capability_source: None,
                    }
                }
                _ => TestConnectionResponse {
                    ok: false,
                    status: "invalid_catalogue".into(),
                    models: vec![],
                    catalog: vec![],
                    hint: Some(
                        "The endpoint responded, but its model catalogue is not OpenAI-compatible. Check the endpoint and provider settings."
                            .into(),
                    ),
                    ..Default::default()
                },
            }
        }
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => TestConnectionResponse {
            ok: false,
            status: "auth_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(
                "The endpoint rejected the credentials. Check the API key and its permissions."
                    .into(),
            ),
            ..Default::default()
        },
        Ok(response) => TestConnectionResponse {
            ok: false,
            status: "http_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(format!(
                "The endpoint returned HTTP {} while loading models.",
                response.status().as_u16()
            )),
            ..Default::default()
        },
        Err(error) if error.is_timeout() => TestConnectionResponse {
            ok: false,
            status: "timeout".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(
                "The endpoint did not respond in time. Check its URL and availability.".into(),
            ),
            ..Default::default()
        },
        Err(_) => TestConnectionResponse {
            ok: false,
            status: "transport_error".into(),
            models: vec![],
            catalog: vec![],
            hint: Some(
                "Kronn could not reach this endpoint. Check the URL and network access.".into(),
            ),
            ..Default::default()
        },
    }
}

/// Fetch a provider-specific catalogue without changing the validity of the
/// chat connection. OpenRouter intentionally serves image and video models on
/// dedicated endpoints; a temporary failure of one optional route must not
/// turn a valid chat credential into a failed connection test. The `bool` is
/// whether the endpoint answered with a parseable catalogue at all — distinct
/// from an empty model list, which is itself proof of "no models" (state C),
/// not "unreachable" (would otherwise be indistinguishable from state B).
async fn fetch_capability_catalogue(
    endpoint: &str,
    api_key: Option<&str>,
    path: &str,
    capability: &str,
) -> (Vec<TestConnectionModel>, bool) {
    let mut request = probe_client().get(format!("{endpoint}{path}"));
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    let Ok(response) = request.send().await else {
        return (Vec::new(), false);
    };
    if !response.status().is_success() {
        return (Vec::new(), false);
    }
    match response.json::<serde_json::Value>().await {
        Ok(body) => match catalog_models_from_body(&body, capability) {
            Some(models) => (models, true),
            None => (Vec::new(), false),
        },
        Err(_) => (Vec::new(), false),
    }
}

/// Whether any entry in a `/v1/models`-shaped body declares
/// `architecture.output_modalities` at all — proof the provider's schema can
/// state a model's output modality, regardless of what that entry says. Its
/// absence across the WHOLE catalogue (not just one entry) means the schema
/// never speaks to modality, so a missing field cannot be read as "chat only":
/// it just was never asked.
fn catalog_declares_modality(body: &serde_json::Value) -> bool {
    body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|model| model["architecture"]["output_modalities"].is_array())
}

fn catalog_models_from_body(
    body: &serde_json::Value,
    default_capability: &str,
) -> Option<Vec<TestConnectionModel>> {
    body["data"].as_array().and_then(|items| {
        items
            .iter()
            .map(|model| {
                let id = model["id"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())?
                    .to_string();
                let display_name = model["name"]
                    .as_str()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or(&id)
                    .to_string();
                let mut capabilities = Vec::new();
                let output_modalities = model["architecture"]["output_modalities"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str);
                for modality in output_modalities {
                    let capability = match modality.to_ascii_lowercase().as_str() {
                        "text" => "chat",
                        "image" => "image",
                        "video" => "video",
                        _ => continue,
                    };
                    if !capabilities.iter().any(|known| known == capability) {
                        capabilities.push(capability.to_string());
                    }
                }
                if capabilities.is_empty() {
                    capabilities.push(default_capability.to_string());
                }
                if model["architecture"]["input_modalities"]
                    .as_array()
                    .is_some_and(|inputs| inputs.iter().any(|input| input == "image"))
                    || model["supports_vision"] == true
                    || model["model_info"]["supports_vision"] == true
                {
                    capabilities.push("vision".to_string());
                }
                Some(TestConnectionModel {
                    id,
                    display_name,
                    capabilities,
                })
            })
            .collect()
    })
}

fn merge_catalog(target: &mut Vec<TestConnectionModel>, discovered: Vec<TestConnectionModel>) {
    for model in discovered {
        if let Some(existing) = target.iter_mut().find(|entry| entry.id == model.id) {
            for capability in model.capabilities {
                if !existing.capabilities.contains(&capability) {
                    existing.capabilities.push(capability);
                }
            }
            if existing.display_name == existing.id && model.display_name != model.id {
                existing.display_name = model.display_name;
            }
        } else {
            target.push(model);
        }
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
            catalog: vec![],
            hint: Some("Enter a valid endpoint before testing the connection.".into()),
            ..Default::default()
        }));
    };
    if reqwest::Url::parse(&endpoint).is_err() {
        return Json(ApiResponse::ok(TestConnectionResponse {
            ok: false,
            status: "invalid_url".into(),
            models: vec![],
            catalog: vec![],
            hint: Some("Enter a valid endpoint before testing the connection.".into()),
            ..Default::default()
        }));
    }

    let config_at_start = state.config.read().await;
    let saved_connection = if req.api_key.is_none() {
        if let Some(connection_id) = req.connection_id.as_ref() {
            let lookup_id = connection_id.clone();
            match state
                .db
                .with_read_conn(move |conn| store::get(conn, &lookup_id))
                .await
            {
                Ok(connection) => connection,
                Err(_) => return Json(ApiResponse::err("Could not read the saved connection")),
            }
        } else {
            None
        }
    } else {
        // An explicitly submitted key (including clearing it) belongs to the
        // draft, not to the saved runtime target.
        None
    };
    if saved_connection.as_ref().is_some_and(|connection| {
        connection.endpoint.as_deref() != Some(endpoint.as_str())
            || req.origin_preset != Some(connection.origin_preset)
    }) {
        return Json(ApiResponse::ok(TestConnectionResponse {
            ok: false,
            status: "credential_required".into(),
            hint: Some(
                "The endpoint or provider changed. Enter the API key again before testing.".into(),
            ),
            ..Default::default()
        }));
    }
    let stored_key = saved_connection.as_ref().and_then(|connection| {
        config_at_start
            .tokens
            .active_key_for(&connection.credential_slug)
            .map(str::to_string)
    });
    drop(config_at_start);
    let key = req
        .api_key
        .as_deref()
        .or(stored_key.as_deref())
        .filter(|key| !key.trim().is_empty());
    // The UI has exactly three tiers. Keep this endpoint bounded even when
    // called directly so one request cannot fan out into arbitrary probes.
    let mut tier_models: Vec<TierModel> = Vec::new();
    for entry in req.tier_models {
        let Some(tier) = ["economy", "default", "reasoning"]
            .into_iter()
            .find(|tier| *tier == entry.tier)
        else {
            continue;
        };
        if tier_models.iter().any(|known| known.tier == tier) {
            continue;
        }
        if let Some(model) = clean(Some(entry.model)) {
            if model.chars().count() <= 256 {
                tier_models.push(TierModel { tier, model });
            }
        }
    }
    let mut requested_models: Vec<String> = Vec::new();
    let candidates: Vec<String> = if tier_models.is_empty() {
        req.models
    } else {
        tier_models
            .iter()
            .map(|entry| entry.model.clone())
            .collect()
    };
    for model in candidates {
        if requested_models.len() == 3 {
            break;
        }
        if let Some(model) = clean(Some(model)) {
            if model.chars().count() <= 256 && !requested_models.contains(&model) {
                requested_models.push(model);
            }
        }
    }
    let mut response = probe_models(
        &endpoint,
        key,
        req.origin_preset,
        &requested_models,
        &tier_models,
    )
    .await;
    // A passing test has proven the key, so every refusal the sweep meets is
    // the model's own.
    if req.origin_preset == Some(ExternalApiConnectionPreset::LiteLlm) && response.ok {
        if let Some((source, modes)) = lite_llm_model_modes(&endpoint, key).await {
            apply_model_modes(&mut response, &modes);
            response.capability_source = Some(source.into());
        }
        if let Some(key) = key {
            response.model_checks = sweep_listed_models(
                &endpoint,
                key,
                &response.catalog,
                &response.tier_checks,
                req.progress_id.as_deref(),
            )
            .await;
        }
    }

    if let Some(connection) = saved_connection {
        // Keep credentials stable through the compare-and-commit boundary;
        // connection edits/deletion are checked inside the same transaction.
        let config = state.config.read().await;
        if config.tokens.active_key_for(&connection.credential_slug) == stored_key.as_deref() {
            let agent_type = match connection.origin_preset {
                ExternalApiConnectionPreset::LiteLlm => AgentType::LiteLlm,
                ExternalApiConnectionPreset::Nvidia => AgentType::Nvidia,
                ExternalApiConnectionPreset::OpenRouter | ExternalApiConnectionPreset::Other => {
                    AgentType::Custom
                }
            };
            let target = catalog_store::http_runtime_target_id(&connection.id);
            let models: Vec<_> = response
                .catalog
                .iter()
                .map(|model| catalog_store::DiscoveredModel {
                    model_id: model.id.clone(),
                    display_name: model.display_name.clone(),
                    resolved_model: None,
                    description: None,
                    capabilities: model.capabilities.clone(),
                    reasoning_modes: Vec::new(),
                    default_reasoning_mode: None,
                })
                .collect();
            let reason = match response.status.as_str() {
                "auth_error" | "credential_required" => ModelUnavailableReason::AuthRequired,
                "timeout" => ModelUnavailableReason::Timeout,
                "invalid_catalogue" => ModelUnavailableReason::InvalidCatalog,
                _ => ModelUnavailableReason::ProviderError,
            };
            let detail = response
                .hint
                .clone()
                .unwrap_or_else(|| response.status.clone());
            let success = response.ok;
            let tier_checks = response.tier_checks.clone();
            let model_checks = response.model_checks.clone();
            let persisted = state
                .db
                .with_conn(move |conn| {
                    let transaction = conn.unchecked_transaction()?;
                    if store::get(&transaction, &connection.id)?.as_ref() != Some(&connection) {
                        return Ok(false);
                    }
                    if success {
                        catalog_store::reconcile_live(&transaction, &target, &agent_type, &models)?;
                        // After the reconcile, so a verdict from a real call
                        // wins over "the proxy lists it" (KT-941).
                        record_tier_verdicts(&transaction, &target, &tier_checks)?;
                        record_model_verdicts(
                            &transaction,
                            &target,
                            model_checks.iter().map(|check| {
                                (
                                    check.model.as_str(),
                                    check.status.as_str(),
                                    check.http_status,
                                )
                            }),
                        )?;
                    } else {
                        catalog_store::record_refresh_failure(
                            &transaction,
                            &target,
                            &agent_type,
                            reason,
                            &detail,
                        )?;
                    }
                    transaction.commit()?;
                    Ok(true)
                })
                .await;
            drop(config);
            match persisted {
                Ok(true) if success => {
                    if crate::core::model_catalog::refresh_runtime_cache(&state.db)
                        .await
                        .is_err()
                    {
                        return Json(ApiResponse::err(
                            "Could not reload the saved connection's model catalog",
                        ));
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!("failed to persist HTTP model catalog probe: {error}");
                    return Json(ApiResponse::err(
                        "Could not persist the saved connection's model catalog",
                    ));
                }
            }
        }
    }

    Json(ApiResponse::ok(response))
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

/// POST /api/external-api/connections/:id/reveal — explicitly reveal the
/// stored credential for the settings eye control. It is intentionally absent
/// from list/edit payloads so ordinary navigation never exposes the secret.
pub async fn reveal_credential(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<String>> {
    let connection = match state
        .db
        .with_read_conn(move |conn| store::get(conn, &id))
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

    let config = state.config.read().await;
    match config.tokens.active_key_for(&connection.credential_slug) {
        Some(value) if !value.trim().is_empty() => Json(ApiResponse::ok(value.to_string())),
        _ => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Credential not found",
        )),
    }
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
    if let Err(message) = validate_credential_format(req.origin_preset, req.api_key.as_deref()) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
    }

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
        image_model: clean(req.image_model),
        video_model: clean(req.video_model),
        media_endpoint: clean(req.media_endpoint),
    };

    if connection.origin_preset == ExternalApiConnectionPreset::LiteLlm {
        let checks = probe_assigned_tier_models(
            connection.endpoint.as_deref().unwrap_or_default(),
            req.api_key.as_deref(),
            &assigned_tier_models(None, &connection),
        )
        .await;
        if let Some(refusal) = unconfirmed_refusal(&checks, req.confirm_unreachable_models) {
            return Json(refusal);
        }
    }

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
    let runtime_changed = store::sync_runtime_config(&connection, &mut cfg);
    if let Some(token) = req.api_key.as_deref() {
        set_credential(&mut cfg, &credential_slug, &display_name, token);
    }
    if runtime_changed || req.api_key.is_some() {
        if let Err(e) = config::save(&cfg).await {
            tracing::warn!("external API connection runtime configuration save failed: {e}");
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
    if let Err(message) = validate_credential_format(req.origin_preset, req.api_key.as_deref()) {
        return Json(ApiResponse::err_coded(ApiErrorCode::Validation, message));
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
        endpoint: Some(endpoint),
        credential_slug: credential_slug.clone(),
        origin_preset: req.origin_preset,
        economy_model: clean(req.economy_model),
        default_model: clean(req.default_model),
        reasoning_model: clean(req.reasoning_model),
        created_at: existing.created_at,
        updated_at: Utc::now(),
        image_model: clean(req.image_model),
        video_model: clean(req.video_model),
        media_endpoint: clean(req.media_endpoint),
    };

    let tier_checks = if updated.origin_preset == ExternalApiConnectionPreset::LiteLlm {
        // The stored key only goes back to the endpoint it was saved for.
        let stored_key = if req.api_key.is_none()
            && existing.endpoint == updated.endpoint
            && existing.origin_preset == updated.origin_preset
        {
            state
                .config
                .read()
                .await
                .tokens
                .active_key_for(&credential_slug)
                .map(str::to_string)
        } else {
            None
        };
        let assigned = assigned_tier_models(Some(&existing), &updated);
        let mut checks = probe_assigned_tier_models(
            updated.endpoint.as_deref().unwrap_or_default(),
            req.api_key.as_deref().or(stored_key.as_deref()),
            &assigned,
        )
        .await;
        // What the catalogue learnt belongs to the endpoint it was learnt on.
        if existing.endpoint == updated.endpoint {
            let target = catalog_store::http_runtime_target_id(&updated.id);
            let known = state
                .db
                .with_read_conn(move |conn| catalog_store::list_for_target(conn, &target))
                .await
                .unwrap_or_default();
            let remembered = remembered_refusals(&known, &assigned, &checks);
            checks.extend(remembered);
        }
        checks
    } else {
        Vec::new()
    };
    if let Some(refusal) = unconfirmed_refusal(&tier_checks, req.confirm_unreachable_models) {
        return Json(refusal);
    }

    let to_update = updated.clone();
    let target = catalog_store::http_runtime_target_id(&updated.id);
    if let Err(e) = state
        .db
        .with_conn(move |conn| {
            store::update(conn, &to_update)?;
            // What the save's own calls proved shows in the pickers at once.
            record_tier_verdicts(conn, &target, &tier_checks)
        })
        .await
    {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            format!("{e}"),
        ));
    }
    if crate::core::model_catalog::refresh_runtime_cache(&state.db)
        .await
        .is_err()
    {
        tracing::warn!("external API connection saved, model catalog cache not reloaded");
    }

    let mut cfg = state.config.write().await;
    let runtime_changed = store::sync_runtime_config(&updated, &mut cfg);
    if let Some(token) = req.api_key.as_deref() {
        set_credential(&mut cfg, &credential_slug, &display_name, token);
    }
    if runtime_changed || req.api_key.is_some() {
        if let Err(e) = config::save(&cfg).await {
            tracing::warn!("external API connection runtime configuration save failed: {e}");
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
    fn http_error_hint_names_the_model_that_failed() {
        // The operator has to know WHICH model answered 400 — a LiteLLM proxy
        // serving embeddings alongside chat returns 400 for the former only.
        let hint = http_error_hint(400, "text-embedding-3-large", false);
        assert!(hint.contains("400"), "{hint}");
        assert!(hint.contains("text-embedding-3-large"), "{hint}");
    }

    #[test]
    fn http_error_hint_keeps_the_nvidia_arms_for_nvidia_only() {
        // 404/410 carry NVIDIA-specific advice, but only when the probe ran
        // against the retained NVIDIA catalogue.
        assert!(http_error_hint(404, "m", true).contains("not accessible to this account"));
        assert!(http_error_hint(410, "m", true).contains("retired"));
        // Same statuses on any other provider must stay generic and named.
        assert!(http_error_hint(404, "m", false).contains("HTTP 404 for model m"));
        assert!(http_error_hint(410, "m", false).contains("HTTP 410 for model m"));
    }

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
            image_model: None,
            video_model: None,
            media_endpoint: None,
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
    fn openrouter_credentials_keep_their_required_prefix() {
        assert!(validate_credential_format(
            ExternalApiConnectionPreset::OpenRouter,
            Some("sk-or-v1-secret")
        )
        .is_ok());
        assert_eq!(
            validate_credential_format(
                ExternalApiConnectionPreset::OpenRouter,
                Some("0123456789abcdef")
            ),
            Err("The OpenRouter API key must include its sk-or-v1- prefix")
        );
        assert!(validate_credential_format(
            ExternalApiConnectionPreset::Nvidia,
            Some("nvapi-secret")
        )
        .is_ok());
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

    #[test]
    fn image_input_declares_vision_independently_of_image_generation() {
        let models = catalog_models_from_body(&serde_json::json!({"data": [
            {"id": "reader", "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text"]}},
            {"id": "generator", "architecture": {"output_modalities": ["image"]}},
            {"id": "declared", "supports_vision": true},
            {"id": "text-only", "supports_vision": false}
        ]}), "chat").unwrap();
        assert_eq!(models[0].capabilities, vec!["chat", "vision"]);
        assert_eq!(models[1].capabilities, vec!["image"]);
        assert_eq!(models[2].capabilities, vec!["chat", "vision"]);
        assert_eq!(models[3].capabilities, vec!["chat"]);
    }

    #[test]
    fn capability_catalogues_merge_without_inventing_media_support() {
        let chat = serde_json::json!({"data": [
            {"id": "shared", "name": "Shared", "architecture": {"output_modalities": ["text"]}},
            {"id": "chat-only"}
        ]});
        let image = serde_json::json!({"data": [
            {"id": "shared", "name": "Shared image", "architecture": {"output_modalities": ["image"]}},
            {"id": "image-only", "name": "Image only"}
        ]});
        let video = serde_json::json!({"data": [
            {"id": "bytedance/seedance", "name": "Seedance"}
        ]});
        let mut catalog = catalog_models_from_body(&chat, "chat").unwrap();
        merge_catalog(
            &mut catalog,
            catalog_models_from_body(&image, "image").unwrap(),
        );
        merge_catalog(
            &mut catalog,
            catalog_models_from_body(&video, "video").unwrap(),
        );

        let shared = catalog.iter().find(|model| model.id == "shared").unwrap();
        assert_eq!(shared.capabilities, vec!["chat", "image"]);
        let chat_only = catalog
            .iter()
            .find(|model| model.id == "chat-only")
            .unwrap();
        assert_eq!(chat_only.capabilities, vec!["chat"]);
        let seedance = catalog
            .iter()
            .find(|model| model.id == "bytedance/seedance")
            .unwrap();
        assert_eq!(seedance.capabilities, vec!["video"]);
    }

    #[test]
    fn catalog_declares_modality_reads_catalog_evidence_not_the_provider_name() {
        // OpenRouter-shaped: at least one entry carries the field.
        let openrouter_like = serde_json::json!({"data": [
            {"id": "text/chat", "architecture": {"output_modalities": ["text"]}},
            {"id": "no-architecture-entry"}
        ]});
        assert!(catalog_declares_modality(&openrouter_like));

        // NVIDIA's real `/v1/models` shape: no entry ever carries the field,
        // regardless of how many chat/image/video models are actually listed.
        let nvidia_like = serde_json::json!({"data": [
            {"id": "meta/llama-3.1-70b-instruct"},
            {"id": "black-forest-labs/flux.1-dev"},
            {"id": "nvidia/cosmos-predict"}
        ]});
        assert!(!catalog_declares_modality(&nvidia_like));

        assert!(!catalog_declares_modality(&serde_json::json!({"data": []})));
        assert!(!catalog_declares_modality(&serde_json::json!({})));
    }

    // ── KT-941 — per-tier verification of a LiteLLM connection ───────────

    const VERTEX_404: &str = "Publisher model `projects/enws-private-project/models/claude-sonnet-5` was not found or your project does not have access";
    const TAGS_401: &str =
        "Not allowed to access model vertex_ai/claude-fable-5 due to tags configuration";

    fn tiers(economy: &str, default: &str, reasoning: &str) -> Vec<TierModel> {
        [
            ("economy", economy),
            ("default", default),
            ("reasoning", reasoning),
        ]
        .into_iter()
        .map(|(tier, model)| TierModel {
            tier,
            model: model.to_string(),
        })
        .collect()
    }

    fn loaded_catalogue() -> TestConnectionResponse {
        TestConnectionResponse {
            ok: true,
            status: "success".into(),
            models: vec!["flash".into(), "vertex_ai/claude-sonnet-5".into()],
            ..Default::default()
        }
    }

    fn http(status: u16, body: &str) -> ChatProbe {
        ChatProbe::Http {
            status,
            body: body.into(),
        }
    }

    fn refused(status: u16, body: &str) -> ChatProbe {
        ChatProbe::Refused {
            status,
            body: body.into(),
        }
    }

    #[test]
    fn each_tier_reports_whether_its_model_answers() {
        // The 01/10 proxy: economy answers, default is a Vertex 404, reasoning
        // is refused by the proxy's tag routing.
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers(
                "flash",
                "vertex_ai/claude-sonnet-5",
                "vertex_ai/claude-fable-5",
            ),
            &[
                ("flash", ChatProbe::Answered),
                ("vertex_ai/claude-sonnet-5", http(404, VERTEX_404)),
                ("vertex_ai/claude-fable-5", refused(401, TAGS_401)),
            ],
        );

        // The connection works and keeps its catalogue: the operator must be
        // able to open the pickers and choose another model.
        assert!(report.ok);
        assert_eq!(report.status, "model_error");
        assert_eq!(report.models.len(), 2);
        let by_tier = |tier: &str| {
            report
                .tier_checks
                .iter()
                .find(|check| check.tier == tier)
                .unwrap()
        };
        assert!(by_tier("economy").ok);
        assert_eq!(by_tier("economy").status, "ok");
        assert_eq!(by_tier("default").status, "not_found");
        assert_eq!(by_tier("default").http_status, Some(404));
        assert_eq!(by_tier("reasoning").status, "access_denied");
        assert_eq!(by_tier("reasoning").http_status, Some(401));
        let hint = report.hint.as_deref().unwrap();
        assert!(hint.contains("vertex_ai/claude-sonnet-5"), "{hint}");
        assert!(hint.contains("vertex_ai/claude-fable-5"), "{hint}");
        assert!(hint.contains("Choose another model"), "{hint}");
    }

    #[test]
    fn the_result_carries_neither_the_upstream_body_nor_a_secret() {
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers(
                "flash",
                "vertex_ai/claude-sonnet-5",
                "vertex_ai/claude-sonnet-5",
            ),
            &[
                ("flash", ChatProbe::Answered),
                ("vertex_ai/claude-sonnet-5", http(404, VERTEX_404)),
            ],
        );
        let wire = serde_json::to_string(&report).unwrap();
        assert!(!wire.contains("enws-private-project"), "{wire}");
        assert!(!wire.contains("Publisher model"), "{wire}");
    }

    #[test]
    fn a_fully_answering_tier_set_is_a_plain_success() {
        let report = assemble_tier_report(
            loaded_catalogue(),
            // Two tiers share one model: it is called once and reported twice.
            &tiers(
                "flash",
                "vertex_ai/claude-sonnet-5",
                "vertex_ai/claude-sonnet-5",
            ),
            &[
                ("flash", ChatProbe::Answered),
                ("vertex_ai/claude-sonnet-5", ChatProbe::Answered),
            ],
        );
        assert!(report.ok);
        assert_eq!(report.status, "success");
        assert_eq!(report.tier_checks.len(), 3);
        assert!(report.tier_checks.iter().all(|check| check.ok));
        assert_eq!(
            report.hint.as_deref(),
            Some("2 configured model(s) answered successfully.")
        );
    }

    #[test]
    fn a_bare_refusal_on_every_model_is_still_a_credential_failure() {
        // Nothing shows the proxy accepted the key: invented per-tier "access
        // denied" verdicts would send the operator hunting the wrong problem.
        let invalid = "Authentication Error, Invalid proxy server token passed";
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers("flash", "other", "other"),
            &[
                ("flash", refused(401, invalid)),
                ("other", refused(401, invalid)),
            ],
        );
        assert!(!report.ok);
        assert_eq!(report.status, "auth_error");
        assert!(report.tier_checks.is_empty());
    }

    #[test]
    fn once_one_model_answers_a_bare_refusal_on_another_is_that_models_allow_list() {
        // The proxy checks the key before the model: an answer elsewhere proves
        // it, so this 403 is about the model.
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers("flash", "other", "other"),
            &[
                ("flash", ChatProbe::Answered),
                ("other", refused(403, "Forbidden")),
            ],
        );
        assert!(report.ok);
        let default = report
            .tier_checks
            .iter()
            .find(|check| check.tier == "default")
            .unwrap();
        assert_eq!(default.status, "access_denied");
        assert_eq!(default.http_status, Some(403));
    }

    #[test]
    fn a_failing_upstream_or_a_slow_one_is_reported_but_never_called_not_found() {
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers("flash", "haiku", "slow"),
            &[
                ("flash", ChatProbe::Answered),
                ("haiku", http(500, "upstream exploded")),
                ("slow", ChatProbe::Timeout),
            ],
        );
        let status = |tier: &str| {
            report
                .tier_checks
                .iter()
                .find(|check| check.tier == tier)
                .map(|check| check.status.as_str())
                .unwrap()
        };
        assert_eq!(status("default"), "http_error");
        assert_eq!(status("reasoning"), "timeout");
    }

    #[test]
    fn a_proxy_that_answers_no_call_is_an_unreachable_connection_not_three_bad_models() {
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers("flash", "other", "other"),
            &[("flash", ChatProbe::Timeout), ("other", ChatProbe::Timeout)],
        );
        assert!(!report.ok);
        assert_eq!(report.status, "timeout");
        assert!(report.tier_checks.is_empty());
    }

    #[test]
    fn a_billing_refusal_stops_the_report() {
        let report = assemble_tier_report(
            loaded_catalogue(),
            &tiers("flash", "other", "other"),
            &[
                ("flash", ChatProbe::Answered),
                ("other", ChatProbe::Billing),
            ],
        );
        assert!(!report.ok);
        assert_eq!(report.status, "billing_error");
    }

    fn catalogue_with(models: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let discovered: Vec<_> = models
            .iter()
            .map(|id| catalog_store::DiscoveredModel {
                model_id: (*id).into(),
                display_name: (*id).into(),
                resolved_model: None,
                description: None,
                capabilities: vec![],
                reasoning_modes: vec![],
                default_reasoning_mode: None,
            })
            .collect();
        catalog_store::reconcile_live(&conn, "http:conn", &AgentType::LiteLlm, &discovered)
            .unwrap();
        conn
    }

    fn check(tier: &str, model: &str, status: &str, http_status: Option<u16>) -> TierCheck {
        TierCheck {
            tier: tier.into(),
            model: model.into(),
            ok: status == "ok",
            status: status.into(),
            http_status,
            hint: None,
        }
    }

    #[test]
    fn tier_verdicts_flag_refused_models_in_the_catalogue_and_clear_recovered_ones() {
        let conn = catalogue_with(&["flash", "gone-404", "tagged-401", "slow"]);
        record_tier_verdicts(
            &conn,
            "http:conn",
            &[
                check("economy", "flash", "ok", None),
                check("default", "gone-404", "not_found", Some(404)),
                check("reasoning", "tagged-401", "access_denied", Some(401)),
                check("economy", "slow", "timeout", None),
            ],
        )
        .unwrap();

        let entries = catalog_store::list_for_target(&conn, "http:conn").unwrap();
        let entry = |id: &str| entries.iter().find(|entry| entry.model_id == id).unwrap();
        assert_eq!(entry("flash").availability, ModelAvailability::Available);
        assert_eq!(entry("slow").availability, ModelAvailability::Available);
        assert_eq!(
            entry("gone-404").availability,
            ModelAvailability::Unavailable
        );
        assert_eq!(
            entry("gone-404").unavailable_reason,
            Some(ModelUnavailableReason::NotFound)
        );
        let detail = entry("gone-404").unavailable_detail.as_deref().unwrap();
        assert!(detail.contains("404"), "{detail}");
        assert!(detail.contains("Choose another model"), "{detail}");
        assert_eq!(
            entry("tagged-401").unavailable_reason,
            Some(ModelUnavailableReason::AccessDenied)
        );

        // The proxy is fixed and the model answers: the flag goes.
        record_tier_verdicts(
            &conn,
            "http:conn",
            &[check("default", "gone-404", "ok", None)],
        )
        .unwrap();
        let entries = catalog_store::list_for_target(&conn, "http:conn").unwrap();
        let recovered = entries
            .iter()
            .find(|entry| entry.model_id == "gone-404")
            .unwrap();
        assert_eq!(recovered.availability, ModelAvailability::Available);
        assert_eq!(recovered.unavailable_reason, None);
    }

    #[test]
    fn model_modes_are_read_from_either_litellm_route() {
        // `/model_group/info` is what a non-admin key may get instead of
        // `/model/info`: flat fields under `model_group`.
        let group = model_modes_from_body(&serde_json::json!({"data": [
            {"model_group": "veo-3", "mode": "video_generation"},
            {"model_group": "imagen-4", "mode": "image_generation"},
            {"model_group": "gemini-3.6-flash", "mode": "chat", "supports_vision": true},
            {"model_group": "text-embedding-005", "mode": "embedding"},
            {"model_group": "no-mode"}
        ]}));
        assert_eq!(group["veo-3"], vec!["video"]);
        assert_eq!(group["imagen-4"], vec!["image"]);
        assert_eq!(group["gemini-3.6-flash"], vec!["chat", "vision"]);
        assert_eq!(group["text-embedding-005"], vec!["embedding"]);
        assert!(!group.contains_key("no-mode"), "no mode, no claim");

        // Two deployments of one alias with different modes keep both.
        let info = model_modes_from_body(&serde_json::json!({"data": [
            {"model_name": "mixed", "model_info": {"mode": "chat"}},
            {"model_name": "mixed", "model_info": {"mode": "image_edit"}}
        ]}));
        assert_eq!(info["mixed"], vec!["chat", "image"]);
        assert!(model_modes_from_body(&serde_json::json!({"error": "forbidden"})).is_empty());
    }

    #[test]
    fn test_progress_ids_are_plain_and_bounded() {
        assert!(valid_progress_id("4f7c1d2e-9a3b-4c5d-8e6f-0a1b2c3d4e5f"));
        for bad in ["", "../x", "a b", &"x".repeat(65)] {
            assert!(!valid_progress_id(bad), "{bad:?}");
        }
    }
}
