//! Images attached to a discussion, as an HTTP agent's model receives them (KT-946).
//!
//! A CLI agent is handed an image's path and opens it with its own tools. An HTTP
//! agent has no such hands: the model only knows what is in the request. Until
//! this module existed the request carried the PATH of the image and nothing
//! else, so a vision model never saw the picture, `read_file` refused the path
//! (it lives in Kronn's data directory, outside the workspace), and the model —
//! told only "you MUST read it" — described a screenshot it had never seen, for
//! four turns, until the user confronted it.
//!
//! Two rules replace that, and both are enforced here rather than left to the
//! model's good faith:
//!
//! * **A model that can see gets the image itself**, in the user message, in the
//!   wire's own spelling (OpenAI: an `image_url` data URL part; Ollama: the
//!   `images` array of base64). Bounded in size and count, and logged.
//! * **A model that cannot — or whose capability nobody knows — is told so, in so
//!   many words**, and told not to describe the picture. A path is never handed
//!   over without that statement; it is not handed over at all, because a path
//!   the model cannot open is exactly what invited the invention.
//!
//! "Can see" is decided from what is known, never guessed from a model name: the
//! catalogue's `vision` capability (an operator can set it by hand) first, then
//! what the provider itself reports (Ollama's `/api/show`, LiteLLM's
//! `/model/info`). An absent answer is `Unknown`, and `Unknown` is treated like
//! `Unsupported`: sending an image to a text-only model is a provider error at
//! best and a silently ignored attachment at worst.

use std::collections::HashSet;

use base64::Engine;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

/// Catalogue capability tag marking a model that accepts images as input. The
/// catalogue's `image` tag means image GENERATION (see `CAPABILITY_CHAT`), so it
/// is deliberately not read here.
pub(crate) const CAPABILITY_VISION: &str = "vision";

/// Largest image sent as is. Providers differ (Anthropic documents 5 MB per
/// image, OpenAI and Gemini accept more). Larger uploads are downscaled before
/// transmission; their original on disk is preserved.
pub(crate) const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 10 * 1024 * 1024;
/// Most images in one request. The newest ones win: a discussion that attached a
/// dozen screenshots is asking about the latest, and the older ones stay listed
/// in the notice so the model knows they exist and cannot be seen.
pub(crate) const MAX_IMAGES: usize = 8;
/// Raw bytes across one request's images (base64 adds a third on the wire).
pub(crate) const MAX_TOTAL_BYTES: usize = 12 * 1024 * 1024;
/// What one image costs in the prompt-size estimates, expressed in bytes of
/// text. Vision models bill an image by its dimensions, not by the bytes of its
/// base64 — a 4 MB screenshot is a few thousand tokens, not a million — so the
/// encoded payload is left out of the estimates and this flat figure (about
/// 1.3-2k tokens) is charged instead. See `messages_wire_len`.
pub(crate) const IMAGE_PROMPT_BYTES: usize = 6_000;

/// One image attached to the discussion, as the dispatcher knows it. `path` is
/// `None` for a legacy row whose bytes were never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextImage {
    pub filename: String,
    pub path: Option<String>,
}

/// Everything a run needs to deliver a discussion's images: the images, the
/// catalogue's word on which models of this target can see, and the room's
/// language for the notice.
#[derive(Debug, Clone, Default)]
pub struct ContextImages {
    /// Oldest first, as attached.
    pub items: Vec<ContextImage>,
    /// Models of the run's target whose catalogue entry declares `vision`.
    pub catalog_vision_models: HashSet<String>,
    pub language: String,
}

/// Whether a model accepts images, and how sure we are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageSupport {
    Supported,
    /// Positively reported as text-only.
    Unsupported,
    /// Nobody said. Treated like `Unsupported` for delivery.
    Unknown,
}

/// Why an image was not sent. Every variant becomes one sentence in the notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Withheld {
    /// The model cannot see, or nobody knows whether it can.
    ModelCannotSee,
    TooLarge {
        bytes: u64,
    },
    /// The request already carries as many images as it may.
    TooMany,
    /// The request's image budget is spent.
    TotalBudget,
    /// Not png/jpeg/gif/webp (SVG, BMP, TIFF, ICO, or bytes that are not an image).
    UnsupportedFormat,
    /// The file is gone or cannot be read.
    Unreadable,
}

/// One image ready for the wire.
#[derive(Debug, Clone)]
pub(crate) struct SentImage {
    pub filename: String,
    pub mime: &'static str,
    pub base64: String,
    pub bytes: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PreparedImages {
    pub sent: Vec<SentImage>,
    /// In attachment order.
    pub withheld: Vec<(String, Withheld)>,
    /// Where the answer to "can it see" came from, for the log.
    pub support_source: &'static str,
    pub total_items: usize,
    language: String,
}

/// Identify an image by its first bytes, never by its extension: a `.png` that is
/// really a JPEG is still deliverable, and a renamed text file is not an image.
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Ollama: `/api/show` lists what the model does in `capabilities`
/// (`["completion", "vision", …]`). A server too old to report it answers
/// without the field, which proves nothing.
pub(crate) fn ollama_show_vision(show: &Value) -> ImageSupport {
    match show.get("capabilities").and_then(Value::as_array) {
        Some(capabilities) if capabilities.iter().any(|c| c.as_str() == Some("vision")) => {
            ImageSupport::Supported
        }
        Some(_) => ImageSupport::Unsupported,
        None => ImageSupport::Unknown,
    }
}

/// LiteLLM: `/model/info` carries `model_info.supports_vision` per deployment.
/// A deployment LiteLLM does not know leaves it false or absent, which is "not
/// declared", not "text-only" — hence `Unknown`, never `Unsupported`.
pub(crate) fn litellm_model_info_vision(info: &Value, alias: &str) -> ImageSupport {
    let declared = info["data"].as_array().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry["model_name"] == alias && entry["model_info"]["supports_vision"] == true
        })
    });
    if declared {
        ImageSupport::Supported
    } else {
        ImageSupport::Unknown
    }
}

/// Ask a LiteLLM-style proxy whether `alias` accepts images. Best-effort, like
/// the rest of `/model/info`: the route is optional and often admin-gated, and
/// any failure is `Unknown`.
pub(crate) async fn litellm_vision(base: &str, api_key: Option<&str>, alias: &str) -> ImageSupport {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return ImageSupport::Unknown;
    };
    let mut request = client.get(format!("{}/model/info", base.trim_end_matches('/')));
    if let Some(key) = api_key.filter(|key| !key.trim().is_empty()) {
        request = request.bearer_auth(key);
    }
    let Ok(response) = request.send().await else {
        return ImageSupport::Unknown;
    };
    if !response.status().is_success() {
        return ImageSupport::Unknown;
    }
    match response.json::<Value>().await {
        Ok(info) => litellm_model_info_vision(&info, alias),
        Err(_) => ImageSupport::Unknown,
    }
}

/// Decide what is sent and what is withheld, reading each file once.
///
/// `provider` is the provider's own answer; the catalogue's `vision` capability
/// for `model` overrides it, because an operator who set it by hand knows
/// something the provider's metadata does not.
pub(crate) async fn prepare(
    images: &ContextImages,
    model: &str,
    provider: ImageSupport,
) -> PreparedImages {
    let mut prepared = PreparedImages {
        total_items: images.items.len(),
        language: images.language.clone(),
        ..Default::default()
    };
    if images.items.is_empty() {
        return prepared;
    }
    let (support, source) = if images.catalog_vision_models.contains(model) {
        (ImageSupport::Supported, "catalogue")
    } else {
        (provider, "provider")
    };
    prepared.support_source = source;
    if support != ImageSupport::Supported {
        for image in &images.items {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::ModelCannotSee));
        }
        tracing::info!(
            target: "kronn::agent::vision",
            model = %model, images = images.items.len(), support = ?support, source,
            "images withheld: the model cannot see them (or its capability is unknown)"
        );
        return prepared;
    }

    let skipped = images.items.len().saturating_sub(MAX_IMAGES);
    let mut total = 0usize;
    for (index, image) in images.items.iter().enumerate() {
        if index < skipped {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::TooMany));
            continue;
        }
        let Some(path) = image.path.as_deref() else {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::Unreadable));
            continue;
        };
        if tokio::fs::symlink_metadata(path)
            .await
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::Unreadable));
            continue;
        }
        match tokio::fs::metadata(path).await {
            Ok(meta) if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES as u64 => {
                prepared.withheld.push((
                    image.filename.clone(),
                    Withheld::TooLarge { bytes: meta.len() },
                ));
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                prepared
                    .withheld
                    .push((image.filename.clone(), Withheld::Unreadable));
                continue;
            }
        }
        let Ok(file) = tokio::fs::File::open(path).await else {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::Unreadable));
            continue;
        };
        let mut bytes = Vec::new();
        if file
            .take(MAX_SOURCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .is_err()
        {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::Unreadable));
            continue;
        }
        // The size was checked from metadata; the read can race a replacement.
        if bytes.len() > MAX_SOURCE_BYTES {
            prepared.withheld.push((
                image.filename.clone(),
                Withheld::TooLarge {
                    bytes: bytes.len() as u64,
                },
            ));
            continue;
        }
        let Some(mut mime) = sniff_image_mime(&bytes) else {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::UnsupportedFormat));
            continue;
        };
        if bytes.len() > MAX_IMAGE_BYTES {
            let original_bytes = bytes.len();
            let resized = tokio::task::spawn_blocking(move || downscale_image(&bytes)).await;
            let Ok(Some(resized)) = resized else {
                prepared.withheld.push((
                    image.filename.clone(),
                    Withheld::TooLarge {
                        bytes: original_bytes as u64,
                    },
                ));
                continue;
            };
            tracing::info!(target: "kronn::agent::vision", image = %image.filename, original_bytes, sent_bytes = resized.len(), "image downscaled for model input");
            bytes = resized;
            mime = "image/png";
        }
        if total + bytes.len() > MAX_TOTAL_BYTES {
            prepared
                .withheld
                .push((image.filename.clone(), Withheld::TotalBudget));
            continue;
        }
        total += bytes.len();
        prepared.sent.push(SentImage {
            filename: image.filename.clone(),
            mime,
            base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            bytes: bytes.len(),
        });
    }
    tracing::info!(
        target: "kronn::agent::vision",
        model = %model, source,
        sent = prepared.sent.len(),
        sent_bytes = total,
        withheld = prepared.withheld.len(),
        images_sent = ?prepared
            .sent
            .iter()
            .map(|image| format!("{} ({}, {} bytes)", image.filename, image.mime, image.bytes))
            .collect::<Vec<_>>(),
        "images attached to the request"
    );
    for (name, reason) in &prepared.withheld {
        tracing::warn!(
            target: "kronn::agent::vision",
            model = %model, image = %name, reason = ?reason,
            "image not sent to the model"
        );
    }
    prepared
}

fn downscale_image(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    for bound in [2048, 1536, 1024, 768] {
        let resized = image.thumbnail(bound, bound);
        let mut output = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut output, image::ImageFormat::Png)
            .ok()?;
        let output = output.into_inner();
        if output.len() <= MAX_IMAGE_BYTES {
            return Some(output);
        }
    }
    None
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

impl PreparedImages {
    /// The size the request's images add to the prompt estimates, in bytes of text.
    pub(crate) fn prompt_bytes(&self) -> usize {
        self.sent.len() * IMAGE_PROMPT_BYTES
    }

    /// The statement the model reads about its attachments, or `""` when the
    /// discussion has none. Every image is accounted for — seen, or explicitly
    /// not — so the model is never left to infer what it holds.
    pub(crate) fn notice(&self) -> String {
        if self.total_items == 0 {
            return String::new();
        }
        let french = self.language == "fr";
        let mut lines: Vec<String> = Vec::new();
        for image in &self.sent {
            lines.push(if french {
                format!(
                    "- `{}` : image jointe au message de l'utilisateur sous forme d'image, tu la vois.",
                    image.filename
                )
            } else {
                format!(
                    "- `{}`: attached to the user's message as an image; you can see it.",
                    image.filename
                )
            });
        }
        for (name, reason) in &self.withheld {
            lines.push(withheld_sentence(name, reason, french));
        }
        format!("=== ATTACHED IMAGES ===\n\n{}", lines.join("\n"))
    }
}

fn withheld_sentence(name: &str, reason: &Withheld, french: bool) -> String {
    match (reason, french) {
        (Withheld::ModelCannotSee, true) => format!(
            "Une image `{name}` est jointe, mais ce modèle ne peut pas la voir : ne décris pas \
             son contenu, demande à l'utilisateur de la décrire ou de changer de modèle."
        ),
        (Withheld::ModelCannotSee, false) => format!(
            "An image `{name}` is attached, but this model cannot see it: do not describe its \
             content, ask the user to describe it or to switch to a model that accepts images."
        ),
        (Withheld::TooLarge { bytes }, true) => format!(
            "Une image `{name}` est jointe ({} Mo), mais elle dépasse la taille transmissible \
             ({} Mo) : ne décris pas son contenu, demande à l'utilisateur d'en joindre une version \
             plus légère ou de la décrire.",
            megabytes(*bytes),
            megabytes(MAX_IMAGE_BYTES as u64)
        ),
        (Withheld::TooLarge { bytes }, false) => format!(
            "An image `{name}` is attached ({} MB) but exceeds the size that can be sent ({} MB): \
             do not describe its content, ask the user to attach a lighter version or to describe it.",
            megabytes(*bytes),
            megabytes(MAX_IMAGE_BYTES as u64)
        ),
        (Withheld::TooMany, true) => format!(
            "Une image `{name}` est jointe, mais seules les {MAX_IMAGES} plus récentes sont \
             transmises : ne décris pas son contenu, demande à l'utilisateur de la rejoindre ou \
             de la décrire."
        ),
        (Withheld::TooMany, false) => format!(
            "An image `{name}` is attached, but only the {MAX_IMAGES} most recent images are \
             sent: do not describe its content, ask the user to attach it again or to describe it."
        ),
        (Withheld::TotalBudget, true) => format!(
            "Une image `{name}` est jointe, mais le volume d'images de cette requête est \
             atteint : ne décris pas son contenu, demande à l'utilisateur de la décrire ou d'en \
             joindre moins."
        ),
        (Withheld::TotalBudget, false) => format!(
            "An image `{name}` is attached, but this request's image budget is spent: do not \
             describe its content, ask the user to describe it or to attach fewer images."
        ),
        (Withheld::UnsupportedFormat, true) => format!(
            "Une image `{name}` est jointe, mais son format n'est pas transmissible (seuls PNG, \
             JPEG, GIF et WebP le sont) : ne décris pas son contenu, demande à l'utilisateur de \
             la décrire ou de la convertir."
        ),
        (Withheld::UnsupportedFormat, false) => format!(
            "An image `{name}` is attached, but its format cannot be sent (only PNG, JPEG, GIF \
             and WebP can): do not describe its content, ask the user to describe it or to \
             convert it."
        ),
        (Withheld::Unreadable, true) => format!(
            "Une image `{name}` est jointe, mais son fichier est introuvable : ne décris pas son \
             contenu, demande à l'utilisateur de la joindre à nouveau."
        ),
        (Withheld::Unreadable, false) => format!(
            "An image `{name}` is attached, but its file cannot be read: do not describe its \
             content, ask the user to attach it again."
        ),
    }
}

/// Put the images into the LAST user message of a chat body, in the wire's own
/// spelling. The prompt of a discussion run is one user message (history plus the
/// new turn), so that message is where the images belong.
///
/// OpenAI-compatible: the `content` string becomes `[text part, image_url
/// parts…]`, each image a `data:` URL. Ollama's native `/api/chat`: the message
/// gains an `images` array of bare base64.
pub(crate) fn attach_to_chat_body(body: &mut Value, openai_wire: bool, sent: &[SentImage]) {
    if sent.is_empty() {
        return;
    }
    let Some(messages) = body["messages"].as_array_mut() else {
        return;
    };
    let Some(user) = messages
        .iter_mut()
        .rev()
        .find(|message| message["role"] == "user")
    else {
        return;
    };
    if openai_wire {
        let text = user["content"].as_str().unwrap_or_default().to_string();
        let mut parts = vec![json!({ "type": "text", "text": text })];
        parts.extend(sent.iter().map(|image| {
            json!({
                "type": "image_url",
                "image_url": { "url": format!("data:{};base64,{}", image.mime, image.base64) },
            })
        }));
        user["content"] = Value::Array(parts);
    } else {
        user["images"] = Value::Array(
            sent.iter()
                .map(|image| Value::String(image.base64.clone()))
                .collect(),
        );
    }
}

/// Length of a chat body's `messages` as the prompt-size estimates should see it:
/// the serialised JSON, with every embedded image's base64 replaced by the flat
/// [`IMAGE_PROMPT_BYTES`]. Counting the payload itself would make one screenshot
/// look like a quarter of a million tokens and trip every context guard.
pub(crate) fn messages_wire_len(body: &Value) -> usize {
    let serialised = body["messages"].to_string().len();
    let Some(messages) = body["messages"].as_array() else {
        return serialised;
    };
    let mut payload = 0usize;
    let mut images = 0usize;
    for message in messages {
        if let Some(natives) = message["images"].as_array() {
            for image in natives {
                payload += image.as_str().map_or(0, str::len);
                images += 1;
            }
        }
        if let Some(parts) = message["content"].as_array() {
            for part in parts {
                if part["type"] == "image_url" {
                    payload += part["image_url"]["url"].as_str().map_or(0, str::len);
                    images += 1;
                }
            }
        }
    }
    serialised.saturating_sub(payload) + images * IMAGE_PROMPT_BYTES
}

#[cfg(test)]
#[path = "vision_test.rs"]
mod tests;
