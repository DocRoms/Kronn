//! Provider abstraction for media generation, sibling to [`ChatCodec`].
//!
//! Why a full URL and not a path: `ChatCodec` derives `{base}/v1/chat/…` from
//! one configured base, which works because chat lives on the configured host.
//! Media does not. OpenRouter serves it from the same base, but NVIDIA serves
//! visual generation from `ai.api.nvidia.com` while its chat endpoint is
//! `integrate.api.nvidia.com` — the value actually stored on the connection.
//! Deriving a path from a single base would therefore build a wrong URL for
//! the second provider, so every method returns a complete URL that the
//! implementation is free to host wherever it must.
//!
//! OpenRouter is NOT OpenAI-compatible here: `/api/v1/images` is synchronous
//! while `/api/v1/videos` is a submit → poll → download cycle. Neither is
//! reachable through `/chat/completions`, which answers a bare HTTP 500.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::agents::media_asset_url::AssetHostPolicy;
use crate::models::{MediaCost, MediaModality, MediaParams, MediaReferenceMode};

/// A source image, already read from disk by the worker.
///
/// Bytes rather than a path or a URL: Kronn runs on `127.0.0.1`, so there is no
/// address a provider could fetch, and publishing one would hand a private file
/// to the internet. The codec decides how its provider wants those bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReferenceImage {
    pub mode: MediaReferenceMode,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

impl MediaReferenceImage {
    /// `data:` form, which is what an inline image looks like on every
    /// OpenAI-shaped route.
    pub fn data_url(&self) -> String {
        use base64::Engine as _;
        format!(
            "data:{};base64,{}",
            self.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }
}

/// Acknowledgement of an asynchronous submission.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaSubmitAck {
    pub provider_job_id: String,
}

/// One observation of an asynchronous job.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaPollState {
    /// Still working. Carries no fabricated progress.
    Pending,
    Completed {
        /// Download URLs, in provider order. They still require the provider
        /// credential, so the download happens server-side.
        urls: Vec<String>,
        cost: Option<MediaCost>,
        generation_id: Option<String>,
    },
    Failed {
        /// Bounded, actionable message. Never a raw payload.
        message: String,
    },
}

/// A synchronous image response: the payloads plus what it cost.
///
/// The cost travels with the images rather than being fetched later: it is the
/// provider's own billed figure for this call, and nothing reconstructs it
/// afterwards.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaImageResponse {
    /// Base64 payloads, still encoded.
    pub images: Vec<String>,
    pub cost: Option<MediaCost>,
    pub generation_id: Option<String>,
}

pub trait MediaCodec: Send + Sync {
    /// Refusing here is what keeps a video model from being dispatched to a
    /// chat endpoint, which is the current failure mode.
    fn supports(&self, modality: MediaModality) -> bool;

    /// Whether this provider takes source images at all, asked BEFORE a job
    /// exists. `image_body`/`video_body` still refuse one that reaches them,
    /// but by then the caller has already queued a generation that can only
    /// fail — and a refusal discovered in the worker reads as a provider
    /// outage, so it is retried until the deadline instead of being told.
    fn accepts_reference_images(&self) -> bool {
        true
    }

    fn image_url(&self, base: &str) -> String;
    fn video_submit_url(&self, base: &str) -> String;
    fn video_poll_url(&self, base: &str, provider_job_id: &str) -> String;
    fn video_content_url(&self, base: &str, provider_job_id: &str, index: u32) -> String;

    /// `references` are the source images the caller pinned, in the order they
    /// were chosen. A codec whose provider takes none must refuse rather than
    /// drop them: an illustration built on nothing, billed in place of the one
    /// that was asked for, is a charge for something nobody wanted.
    fn image_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        references: &[MediaReferenceImage],
    ) -> Result<Value>;
    /// `reference` is the source image the caller pinned, when there is one.
    /// A codec whose provider cannot take one must refuse rather than drop it
    /// silently: a text-to-video billed in place of the requested
    /// image-to-video is a charge for something nobody asked for.
    fn video_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        reference: Option<&MediaReferenceImage>,
    ) -> Result<Value>;

    /// Payloads and billed cost of a synchronous image response.
    fn parse_image_response(&self, body: &str) -> Result<MediaImageResponse>;
    fn parse_submit_response(&self, body: &str) -> Result<MediaSubmitAck>;
    fn parse_poll_response(&self, body: &str) -> Result<MediaPollState>;

    /// Hosts this provider may serve assets from. Consulted before any
    /// provider-supplied URL is fetched, and before the credential is
    /// attached to one.
    fn asset_host_policy(&self) -> AssetHostPolicy;
}

/// Reads the provider's billed cost. Persisted verbatim, BYOK flag included:
/// a BYOK call bills zero here while the real spend sits with the upstream
/// account, and averaging that zero into an estimate would understate it.
fn usage_cost(value: &Value) -> Option<MediaCost> {
    value.get("usage").map(|usage| MediaCost {
        cost_usd: usage.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        is_byok: usage
            .get("is_byok")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn trimmed_base(base: &str) -> &str {
    base.trim_end_matches('/')
}

/// Base for a provider whose media routes live under `/v1`.
///
/// Operators legitimately store either `https://openrouter.ai/api` or
/// `.../api/v1` as the connection endpoint — both work for chat. Appending
/// `/v1/images` to the second one would POST to `/api/v1/v1/images` and fail
/// with a 404 that looks like a provider outage.
fn base_without_v1(base: &str) -> &str {
    trimmed_base(base).trim_end_matches("/v1")
}

/// Bounds a provider error so a raw payload never reaches storage or the UI.
fn bounded(message: &str) -> String {
    const MAX: usize = 300;
    let cleaned = message.trim();
    if cleaned.chars().count() <= MAX {
        return cleaned.to_string();
    }
    cleaned.chars().take(MAX).collect::<String>() + "…"
}

pub struct OpenRouterMediaCodec;

impl MediaCodec for OpenRouterMediaCodec {
    fn supports(&self, _modality: MediaModality) -> bool {
        true
    }

    fn image_url(&self, base: &str) -> String {
        format!("{}/v1/images", base_without_v1(base))
    }

    fn video_submit_url(&self, base: &str) -> String {
        format!("{}/v1/videos", base_without_v1(base))
    }

    fn video_poll_url(&self, base: &str, provider_job_id: &str) -> String {
        format!("{}/v1/videos/{}", base_without_v1(base), provider_job_id)
    }

    fn video_content_url(&self, base: &str, provider_job_id: &str, index: u32) -> String {
        format!(
            "{}/v1/videos/{}/content?index={}",
            base_without_v1(base),
            provider_job_id,
            index
        )
    }

    fn image_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        references: &[MediaReferenceImage],
    ) -> Result<Value> {
        let mut body = json!({ "model": model, "prompt": prompt });
        if let Some(resolution) = &params.resolution {
            body["resolution"] = json!(resolution);
        }
        if let Some(ratio) = &params.aspect_ratio {
            body["aspect_ratio"] = json!(ratio);
        }
        if !references.is_empty() {
            // `input_references` is the documented shape of the images route:
            // the same object as a video's `frame_images`, minus `frame_type`.
            // A frame is a picture the clip must reproduce at one end, which
            // an illustration has no equivalent of — so asking for one here is
            // refused rather than quietly turned into a plain reference.
            if let Some(frame) = references.iter().find(|r| r.mode.frame_capability().is_some()) {
                bail!(
                    "an image generation takes reference images, not a '{}'",
                    frame.mode.as_str()
                );
            }
            body["input_references"] = json!(references
                .iter()
                .map(|reference| json!({
                    "type": "image_url",
                    "image_url": { "url": reference.data_url() },
                }))
                .collect::<Vec<_>>());
        }
        Ok(body)
    }

    fn video_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        reference: Option<&MediaReferenceImage>,
    ) -> Result<Value> {
        let mut body = json!({ "model": model, "prompt": prompt });
        if let Some(duration) = params.duration_secs {
            body["duration"] = json!(duration);
        }
        if let Some(resolution) = &params.resolution {
            body["resolution"] = json!(resolution);
        }
        if let Some(ratio) = &params.aspect_ratio {
            body["aspect_ratio"] = json!(ratio);
        }
        if let Some(audio) = params.generate_audio {
            body["generate_audio"] = json!(audio);
        }
        if let Some(reference) = reference {
            // `frame_images` is the documented shape, and `frame_type` is what
            // separates a first frame from a last one. A visual reference is
            // NOT a frame: no video model advertises it, so it is refused
            // here rather than submitted as a frame the model would honour
            // differently from what was asked.
            let Some(frame_type) = reference.mode.frame_capability() else {
                bail!(
                    "this provider takes a first or last frame, not a '{}'",
                    reference.mode.as_str()
                );
            };
            body["frame_images"] = json!([{
                "type": "image_url",
                "image_url": { "url": reference.data_url() },
                "frame_type": frame_type,
            }]);
        }
        Ok(body)
    }

    fn parse_image_response(&self, body: &str) -> Result<MediaImageResponse> {
        let value: Value = serde_json::from_str(body)?;
        if let Some(message) = provider_error(&value) {
            bail!("{}", bounded(&message));
        }
        let images: Vec<String> = value
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("b64_json").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if images.is_empty() {
            bail!("image response carries no b64_json payload");
        }
        Ok(MediaImageResponse {
            images,
            cost: usage_cost(&value),
            generation_id: value.get("id").and_then(Value::as_str).map(str::to_string),
        })
    }

    fn parse_submit_response(&self, body: &str) -> Result<MediaSubmitAck> {
        let value: Value = serde_json::from_str(body)?;
        if let Some(message) = provider_error(&value) {
            bail!("{}", bounded(&message));
        }
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("submission carries no job id"))?;
        Ok(MediaSubmitAck {
            provider_job_id: id.to_string(),
        })
    }

    fn asset_host_policy(&self) -> AssetHostPolicy {
        // `/videos/{id}/content` is served by the API host and requires the
        // key; OpenRouter hands back no third-party storage URL today.
        AssetHostPolicy::credentialed_only(&["openrouter.ai"])
    }
    fn parse_poll_response(&self, body: &str) -> Result<MediaPollState> {
        let value: Value = serde_json::from_str(body)?;
        if let Some(message) = provider_error(&value) {
            return Ok(MediaPollState::Failed {
                message: bounded(&message),
            });
        }
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match status {
            "completed" | "succeeded" => {
                let urls: Vec<String> = value
                    .get("unsigned_urls")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if urls.is_empty() {
                    bail!("completed job carries no download url");
                }
                // Cost is taken verbatim: the published rate does not
                // reproduce the billed amount.
                let cost = usage_cost(&value);
                let generation_id = value
                    .get("generation_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Ok(MediaPollState::Completed {
                    urls,
                    cost,
                    generation_id,
                })
            }
            "failed" | "cancelled" | "error" => Ok(MediaPollState::Failed {
                message: bounded(&format!("provider reported status {status}")),
            }),
            "" => bail!("poll response carries no status"),
            _ => Ok(MediaPollState::Pending),
        }
    }
}

/// In-band error on an otherwise successful HTTP response.
fn provider_error(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| error.as_str().map(str::to_string))
        .or_else(|| Some(error.to_string()))
}

/// NVIDIA Visual GenAI.
///
/// The reason `MediaCodec` returns a COMPLETE URL rather than a path lives
/// here: NVIDIA serves visual generation from `ai.api.nvidia.com` while a
/// connection stores `integrate.api.nvidia.com` for chat. Deriving a path from
/// the configured base would build a wrong URL for this provider, which is
/// exactly what this second implementation proves the trait avoids.
pub struct NvidiaMediaCodec;

/// Host serving visual generation, independent of the configured chat base.
const NVIDIA_VISUAL_HOST: &str = "https://ai.api.nvidia.com";

impl NvidiaMediaCodec {
    /// The visual host, unless the connection overrides it — a self-hosted NIM
    /// lives wherever the client put it, so the override is the only way to
    /// reach it.
    fn host(base: &str) -> String {
        let base = trimmed_base(base);
        if base.contains("integrate.api.nvidia.com") || base.is_empty() {
            NVIDIA_VISUAL_HOST.to_string()
        } else {
            base.to_string()
        }
    }
}

impl MediaCodec for NvidiaMediaCodec {
    fn accepts_reference_images(&self) -> bool {
        false
    }

    fn supports(&self, modality: MediaModality) -> bool {
        // Both are served, but through the OpenAI-compatible visual routes
        // rather than the proprietary shapes OpenRouter uses.
        matches!(modality, MediaModality::Image | MediaModality::Video)
    }

    fn image_url(&self, base: &str) -> String {
        format!("{}/v1/images/generations", Self::host(base))
    }

    fn video_submit_url(&self, base: &str) -> String {
        format!("{}/v1/videos/generations", Self::host(base))
    }

    fn video_poll_url(&self, base: &str, provider_job_id: &str) -> String {
        format!(
            "{}/v1/videos/generations/{}",
            Self::host(base),
            provider_job_id
        )
    }

    fn video_content_url(&self, base: &str, provider_job_id: &str, index: u32) -> String {
        format!(
            "{}/v1/videos/generations/{}/content?index={}",
            Self::host(base),
            provider_job_id,
            index
        )
    }

    fn image_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        references: &[MediaReferenceImage],
    ) -> Result<Value> {
        // Same reason as the video route below: nobody here has an NVIDIA
        // image slot to measure the reference contract against, and a guessed
        // payload would be submitted and billed on a supposition.
        if !references.is_empty() {
            bail!("generating from reference images is not supported on this NVIDIA connection yet");
        }
        let mut body = json!({ "model": model, "prompt": prompt });
        if let Some(resolution) = &params.resolution {
            body["size"] = json!(resolution);
        }
        Ok(body)
    }

    fn video_body(
        &self,
        model: &str,
        prompt: &str,
        params: &MediaParams,
        reference: Option<&MediaReferenceImage>,
    ) -> Result<Value> {
        // NVIDIA's image-to-video contract is not the OpenAI-shaped one, and
        // nobody here has an NVIDIA video slot to measure it against. Guessing
        // a payload would submit — and bill — a request built on a supposition.
        // Refusing names what is missing instead.
        if reference.is_some() {
            bail!("generating from a source image is not supported on this NVIDIA connection yet");
        }
        let mut body = json!({ "model": model, "prompt": prompt });
        if let Some(duration) = params.duration_secs {
            body["duration"] = json!(duration);
        }
        if let Some(resolution) = &params.resolution {
            body["size"] = json!(resolution);
        }
        Ok(body)
    }

    fn parse_image_response(&self, body: &str) -> Result<MediaImageResponse> {
        // Same OpenAI-compatible envelope as OpenRouter's image route.
        OpenRouterMediaCodec.parse_image_response(body)
    }

    fn parse_submit_response(&self, body: &str) -> Result<MediaSubmitAck> {
        let value: Value = serde_json::from_str(body)?;
        if let Some(message) = provider_error(&value) {
            bail!("{}", bounded(&message));
        }
        // NVIDIA returns the handle under `id`, and some routes echo it as
        // `request_id`; accepting both avoids a resubmission that would bill
        // twice for the same clip.
        let id = value
            .get("id")
            .or_else(|| value.get("request_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("submission carries no job id"))?;
        Ok(MediaSubmitAck {
            provider_job_id: id.to_string(),
        })
    }

    fn asset_host_policy(&self) -> AssetHostPolicy {
        // Nvidia serves visual results from its own hosts (`ai.api.` for the
        // visual API, `integrate.api.` for the gateway). Deliberately no
        // blanket storage suffix: should Nvidia start returning pre-signed
        // third-party URLs, the job fails naming the host instead of quietly
        // fetching from it.
        AssetHostPolicy::credentialed_only(&[".nvidia.com"])
    }

    fn parse_poll_response(&self, body: &str) -> Result<MediaPollState> {
        let value: Value = serde_json::from_str(body)?;
        if let Some(message) = provider_error(&value) {
            return Ok(MediaPollState::Failed {
                message: bounded(&message),
            });
        }
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match status {
            "completed" | "succeeded" | "success" => {
                let urls: Vec<String> = value
                    .get("data")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.get("url").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if urls.is_empty() {
                    bail!("completed job carries no download url");
                }
                let cost = usage_cost(&value);
                Ok(MediaPollState::Completed {
                    urls,
                    cost,
                    generation_id: value.get("id").and_then(Value::as_str).map(str::to_string),
                })
            }
            "failed" | "cancelled" | "error" => Ok(MediaPollState::Failed {
                message: bounded(&format!("provider reported status {status}")),
            }),
            "" => bail!("poll response carries no status"),
            _ => Ok(MediaPollState::Pending),
        }
    }
}

/// The codec a connection's provider needs. Both the submission path and the
/// worker resolve it here: a preflight that guessed differently from the
/// worker would refuse the wrong requests, or let the wrong ones through.
pub fn codec_for(preset: crate::models::ExternalApiConnectionPreset) -> Box<dyn MediaCodec> {
    match preset {
        crate::models::ExternalApiConnectionPreset::Nvidia => Box::new(NvidiaMediaCodec),
        _ => Box::new(OpenRouterMediaCodec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from a real generation on bytedance/seedance-2.0-mini.
    const SUBMIT: &str = r#"{"id":"Nu908SAyQg81UNYgC4Xh","polling_url":"https://openrouter.ai/api/v1/videos/Nu908SAyQg81UNYgC4Xh","status":"pending"}"#;
    const POLL_DONE: &str = r#"{"id":"Nu908SAyQg81UNYgC4Xh","generation_id":"gen-vid-1788188104-R9pvv93wrYQ9a0Kn8WyN","polling_url":"https://openrouter.ai/api/v1/videos/Nu908SAyQg81UNYgC4Xh","status":"completed","unsigned_urls":["https://openrouter.ai/api/v1/videos/Nu908SAyQg81UNYgC4Xh/content?index=0"],"usage":{"cost":0.0708932,"is_byok":false}}"#;

    fn codec() -> OpenRouterMediaCodec {
        OpenRouterMediaCodec
    }

    #[test]
    fn both_endpoint_shapes_an_operator_stores_reach_the_same_route() {
        // Kronn's own OpenRouter connection stores `.../api`; the docs show
        // `.../api/v1`. Chat works with either, so media must too.
        for base in [
            "https://openrouter.ai/api",
            "https://openrouter.ai/api/",
            "https://openrouter.ai/api/v1",
            "https://openrouter.ai/api/v1/",
        ] {
            assert_eq!(
                OpenRouterMediaCodec.image_url(base),
                "https://openrouter.ai/api/v1/images",
                "base {base} must not double the version segment"
            );
            assert_eq!(
                OpenRouterMediaCodec.video_submit_url(base),
                "https://openrouter.ai/api/v1/videos"
            );
        }
    }

    #[test]
    fn urls_are_complete_and_tolerate_a_trailing_slash() {
        let c = codec();
        assert_eq!(
            c.image_url("https://openrouter.ai/api"),
            "https://openrouter.ai/api/v1/images"
        );
        assert_eq!(
            c.video_submit_url("https://openrouter.ai/api/"),
            "https://openrouter.ai/api/v1/videos"
        );
        assert_eq!(
            c.video_poll_url("https://openrouter.ai/api", "job1"),
            "https://openrouter.ai/api/v1/videos/job1"
        );
        assert_eq!(
            c.video_content_url("https://openrouter.ai/api", "job1", 0),
            "https://openrouter.ai/api/v1/videos/job1/content?index=0"
        );
        // Never the chat endpoint: that path answers HTTP 500 for these models.
        assert!(!c
            .video_submit_url("https://openrouter.ai/api")
            .contains("chat/completions"));
    }

    #[test]
    fn a_real_submission_yields_its_job_id() {
        let ack = codec().parse_submit_response(SUBMIT).unwrap();
        assert_eq!(ack.provider_job_id, "Nu908SAyQg81UNYgC4Xh");
    }

    #[test]
    fn a_real_completion_yields_url_declared_cost_and_generation_id() {
        match codec().parse_poll_response(POLL_DONE).unwrap() {
            MediaPollState::Completed {
                urls,
                cost,
                generation_id,
            } => {
                assert_eq!(urls.len(), 1);
                assert!(urls[0].ends_with("/content?index=0"));
                let cost = cost.expect("usage present");
                // Verbatim: rate x duration would give 0.0678 for this clip.
                assert_eq!(cost.cost_usd, 0.070_893_2);
                assert!(!cost.is_byok);
                assert_eq!(
                    generation_id.as_deref(),
                    Some("gen-vid-1788188104-R9pvv93wrYQ9a0Kn8WyN")
                );
            }
            other => panic!("expected completion, got {other:?}"),
        }
    }

    #[test]
    fn a_pending_poll_is_pending_and_an_unknown_status_is_not_a_success() {
        let c = codec();
        assert_eq!(
            c.parse_poll_response(SUBMIT).unwrap(),
            MediaPollState::Pending
        );
        // An unrecognised status must never be optimistically completed.
        let odd = r#"{"id":"x","status":"queued_somewhere"}"#;
        assert_eq!(c.parse_poll_response(odd).unwrap(), MediaPollState::Pending);
    }

    #[test]
    fn a_completed_job_without_a_url_is_an_error_not_a_silent_success() {
        let body = r#"{"id":"x","status":"completed","usage":{"cost":1.0}}"#;
        assert!(codec().parse_poll_response(body).is_err());
    }

    #[test]
    fn a_missing_status_is_refused_rather_than_guessed() {
        assert!(codec().parse_poll_response(r#"{"id":"x"}"#).is_err());
    }

    #[test]
    fn provider_errors_are_surfaced_and_bounded() {
        let long = "x".repeat(5_000);
        let body = format!(r#"{{"error":{{"message":"{long}"}}}}"#);
        match codec().parse_poll_response(&body).unwrap() {
            MediaPollState::Failed { message } => {
                assert!(
                    message.chars().count() <= 301,
                    "len {}",
                    message.chars().count()
                );
                assert!(message.ends_with('…'));
            }
            other => panic!("expected failure, got {other:?}"),
        }
        // On the synchronous path an in-band error is a hard error.
        assert!(codec()
            .parse_image_response(r#"{"error":{"message":"no such model"}}"#)
            .is_err());
    }

    #[test]
    fn image_responses_carry_base64_not_urls() {
        let body = r#"{"id":"gen-img-1","data":[{"b64_json":"AAAA"},{"b64_json":"BBBB"}],"usage":{"cost":0.01,"is_byok":false}}"#;
        let parsed = codec().parse_image_response(body).unwrap();
        assert_eq!(parsed.images, vec!["AAAA", "BBBB"]);
        // The billed cost must survive parsing: images are charged too, and
        // this response is the only place the figure appears.
        assert_eq!(
            parsed.cost,
            Some(MediaCost {
                cost_usd: 0.01,
                is_byok: false
            })
        );
        assert_eq!(parsed.generation_id.as_deref(), Some("gen-img-1"));

        // BYOK bills zero here while the real spend sits upstream; the flag is
        // what keeps that zero out of an average.
        let byok = codec()
            .parse_image_response(
                r#"{"data":[{"b64_json":"AAAA"}],"usage":{"cost":0,"is_byok":true}}"#,
            )
            .unwrap();
        assert_eq!(
            byok.cost,
            Some(MediaCost {
                cost_usd: 0.0,
                is_byok: true
            })
        );
        // A data array with no payload is an error, not an empty success.
        assert!(codec()
            .parse_image_response(r#"{"data":[{"url":"http://x"}]}"#)
            .is_err());
    }

    #[test]
    fn nvidia_targets_its_own_visual_host_not_the_configured_chat_base() {
        let c = NvidiaMediaCodec;
        // THE reason the trait returns a URL and not a path: the connection
        // stores the chat host, which does not serve visual generation.
        let configured = "https://integrate.api.nvidia.com";
        assert!(c
            .image_url(configured)
            .starts_with("https://ai.api.nvidia.com/"));
        assert!(c
            .video_submit_url(configured)
            .starts_with("https://ai.api.nvidia.com/"));
        assert!(!c.video_poll_url(configured, "j").contains("integrate.api"));

        // A self-hosted NIM lives wherever the client put it, so an explicit
        // override must win.
        let self_hosted = "https://nim.internal:8000";
        assert!(c
            .image_url(self_hosted)
            .starts_with("https://nim.internal:8000/"));
    }

    #[test]
    fn nvidia_accepts_both_handle_field_names() {
        let c = NvidiaMediaCodec;
        // Resubmitting because a handle was not recognised would bill twice.
        assert_eq!(
            c.parse_submit_response(r#"{"id":"abc"}"#)
                .unwrap()
                .provider_job_id,
            "abc"
        );
        assert_eq!(
            c.parse_submit_response(r#"{"request_id":"def"}"#)
                .unwrap()
                .provider_job_id,
            "def"
        );
        assert!(c.parse_submit_response(r#"{"nothing":1}"#).is_err());
    }

    #[test]
    fn nvidia_reads_its_own_completion_shape_and_refuses_a_urlless_success() {
        let c = NvidiaMediaCodec;
        let done = r#"{"id":"gen-1","status":"succeeded","data":[{"url":"https://ai.api.nvidia.com/x"}],"usage":{"cost":0.02}}"#;
        match c.parse_poll_response(done).unwrap() {
            MediaPollState::Completed {
                urls,
                cost,
                generation_id,
            } => {
                assert_eq!(urls.len(), 1);
                assert_eq!(cost.unwrap().cost_usd, 0.02);
                assert_eq!(generation_id.as_deref(), Some("gen-1"));
            }
            other => panic!("expected completion, got {other:?}"),
        }
        // Same refusals as the other provider: no silent empty success.
        assert!(c.parse_poll_response(r#"{"status":"succeeded"}"#).is_err());
        assert!(c.parse_poll_response(r#"{"id":"x"}"#).is_err());
        assert_eq!(
            c.parse_poll_response(r#"{"status":"queued"}"#).unwrap(),
            MediaPollState::Pending
        );
    }

    #[test]
    fn the_trait_holds_for_two_providers_without_being_changed() {
        // The point of lot 7: a second implementation must not require the
        // trait to grow.
        let codecs: Vec<Box<dyn MediaCodec>> =
            vec![Box::new(OpenRouterMediaCodec), Box::new(NvidiaMediaCodec)];
        for codec in &codecs {
            assert!(codec.supports(MediaModality::Video));
            let url = codec.video_submit_url("https://integrate.api.nvidia.com");
            assert!(url.starts_with("https://"), "{url}");
            // Neither provider may route media through the chat endpoint.
            assert!(!url.contains("chat/completions"), "{url}");
        }
    }

    #[test]
    fn bodies_only_carry_the_parameters_that_were_set() {
        let c = codec();
        let params = MediaParams {
            duration_secs: Some(5),
            resolution: Some("480p".into()),
            aspect_ratio: Some("16:9".into()),
            generate_audio: Some(false),
            ..MediaParams::default()
        };
        let body = c
            .video_body("bytedance/seedance-2.0-mini", "un chat", &params, None)
            .unwrap();
        assert_eq!(body["duration"], 5);
        assert_eq!(body["generate_audio"], false);
        // No source image was pinned, so the body must not mention one.
        assert!(body.get("frame_images").is_none());

        // An empty params set must not invent defaults the provider would
        // then bill differently from what the user saw.
        let bare = c
            .video_body("m", "p", &MediaParams::default(), None)
            .unwrap();
        assert!(bare.get("duration").is_none());
        assert!(bare.get("generate_audio").is_none());
        assert_eq!(bare["model"], "m");
    }

    fn reference(mode: MediaReferenceMode) -> MediaReferenceImage {
        MediaReferenceImage {
            mode,
            mime_type: "image/png".into(),
            // Two bytes are enough: what matters is the envelope, not the
            // picture.
            bytes: vec![0x89, 0x50],
        }
    }

    #[test]
    fn a_source_image_travels_inline_never_as_an_address() {
        // Kronn listens on 127.0.0.1: there is no URL a provider could fetch,
        // and publishing one would hand a private file to the internet.
        let body = OpenRouterMediaCodec
            .video_body(
                "bytedance/seedance-2.0-mini",
                "un renard",
                &MediaParams::default(),
                Some(&reference(MediaReferenceMode::FirstFrame)),
            )
            .unwrap();
        let frames = body["frame_images"].as_array().expect("one frame image");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["frame_type"], "first_frame");
        assert_eq!(frames[0]["type"], "image_url");
        let url = frames[0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "got {url}");
        assert!(!url.contains("http"), "no address of ours may leave: {url}");
    }

    #[test]
    fn several_reference_images_travel_inline_in_the_order_they_were_chosen() {
        // Measured shape of `POST /v1/images`: the same object as a video's
        // `frame_images`, minus `frame_type`. Documented as accepting base64
        // data URLs, which is the only form Kronn can send — it listens on
        // 127.0.0.1, so there is no address a provider could fetch.
        let first = MediaReferenceImage {
            mode: MediaReferenceMode::Reference,
            mime_type: "image/png".into(),
            bytes: vec![0x01],
        };
        let second = MediaReferenceImage {
            mode: MediaReferenceMode::Reference,
            mime_type: "image/jpeg".into(),
            bytes: vec![0x02],
        };
        let body = OpenRouterMediaCodec
            .image_body(
                "google/gemini-3-pro-image",
                "un renard",
                &MediaParams::default(),
                &[first, second],
            )
            .unwrap();
        let references = body["input_references"]
            .as_array()
            .expect("two reference images");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0]["type"], "image_url");
        // Order is what the caller chose, and providers weigh references by
        // position — reordering them would change the result silently.
        assert!(references[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(references[1]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/jpeg;base64,"));
        // A frame is a video notion; nothing of it may leak into this body.
        assert!(body.get("frame_images").is_none());
        assert!(!body.to_string().contains("frame_type"));
    }

    #[test]
    fn an_image_without_references_says_nothing_about_them() {
        let body = OpenRouterMediaCodec
            .image_body("m", "p", &MediaParams::default(), &[])
            .unwrap();
        assert!(body.get("input_references").is_none());
        assert_eq!(body["model"], "m");
    }

    #[test]
    fn a_frame_is_refused_by_the_image_route_rather_than_turned_into_a_reference() {
        // An illustration has no first or last frame. Silently demoting the
        // request to a plain reference would bill a generation nobody asked
        // for, and the difference would only show in the result.
        let error = OpenRouterMediaCodec
            .image_body(
                "google/gemini-3-pro-image",
                "p",
                &MediaParams::default(),
                &[reference(MediaReferenceMode::FirstFrame)],
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("first_frame"), "got {error}");
    }

    #[test]
    fn nvidia_refuses_reference_images_instead_of_guessing_their_shape() {
        // Nobody here has an NVIDIA image slot to measure the contract
        // against, and a guessed payload would be submitted and billed.
        let error = NvidiaMediaCodec
            .image_body(
                "m",
                "p",
                &MediaParams::default(),
                &[reference(MediaReferenceMode::Reference)],
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("not supported"), "got {error}");
        assert!(NvidiaMediaCodec
            .image_body("m", "p", &MediaParams::default(), &[])
            .is_ok());
    }

    #[test]
    fn the_last_frame_is_pinned_as_the_last_one() {
        let body = OpenRouterMediaCodec
            .video_body(
                "m",
                "p",
                &MediaParams::default(),
                Some(&reference(MediaReferenceMode::LastFrame)),
            )
            .unwrap();
        assert_eq!(body["frame_images"][0]["frame_type"], "last_frame");
    }

    #[test]
    fn a_visual_reference_is_refused_rather_than_passed_off_as_a_frame() {
        // No video model advertises "reference" under `supported_frame_images`.
        // Submitting it as a frame would have the model reproduce a picture the
        // caller only meant as inspiration — and bill for it.
        let error = OpenRouterMediaCodec
            .video_body(
                "m",
                "p",
                &MediaParams::default(),
                Some(&reference(MediaReferenceMode::Reference)),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("first or last frame"), "got {error}");
    }

    #[test]
    fn nvidia_names_what_it_cannot_do_instead_of_guessing_a_payload() {
        // Nobody here has an NVIDIA video slot to measure its image-to-video
        // contract against, and an invented payload would be submitted, and
        // billed, on a supposition.
        let error = NvidiaMediaCodec
            .video_body(
                "m",
                "p",
                &MediaParams::default(),
                Some(&reference(MediaReferenceMode::FirstFrame)),
            )
            .unwrap_err()
            .to_string();
        assert!(error.contains("not supported"), "got {error}");
        // Without a source image it keeps working exactly as before.
        assert!(NvidiaMediaCodec
            .video_body("m", "p", &MediaParams::default(), None)
            .is_ok());
    }

    #[test]
    fn a_provider_that_takes_no_source_image_says_so_before_a_job_exists() {
        // The bodies already refuse a reference that reaches them, but by then
        // the generation is queued and the refusal reads as an outage. This is
        // the same knowledge, askable while there is still nothing to cancel.
        assert!(!NvidiaMediaCodec.accepts_reference_images());
        assert!(OpenRouterMediaCodec.accepts_reference_images());

        // The declaration and the bodies must not drift apart: a codec that
        // announced acceptance and then refused would queue jobs that can only
        // fail, which is exactly the bug this pair prevents.
        let refused = NvidiaMediaCodec.video_body(
            "m",
            "p",
            &MediaParams::default(),
            Some(&reference(MediaReferenceMode::FirstFrame)),
        );
        assert!(refused.is_err());
    }

    #[test]
    fn the_codec_selector_follows_the_connection_preset() {
        use crate::models::ExternalApiConnectionPreset;
        // The submission path and the worker both resolve the codec here. If
        // they disagreed, a preflight would clear what the worker refuses.
        assert!(!codec_for(ExternalApiConnectionPreset::Nvidia).accepts_reference_images());
        assert!(codec_for(ExternalApiConnectionPreset::OpenRouter).accepts_reference_images());
        assert!(codec_for(ExternalApiConnectionPreset::Other).accepts_reference_images());
    }
}
