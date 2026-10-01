//! KT-946 — what a model is given of an attached image, and what it is told when
//! it is given nothing.

use super::*;
use base64::Engine;
use serde_json::json;

/// The eight-byte PNG signature plus filler: enough for the sniffer, which reads
/// only the leading bytes, and small enough to keep tests quick.
fn png_bytes(extra: usize) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    bytes.extend(std::iter::repeat_n(7u8, extra));
    bytes
}

/// A `context-files` directory holding the given files, the way Kronn lays out
/// attachments, and the images that point at them.
fn attached(dir: &tempfile::TempDir, files: &[(&str, Vec<u8>)]) -> ContextImages {
    let storage = dir.path().join("context-files");
    std::fs::create_dir_all(&storage).unwrap();
    let items = files
        .iter()
        .map(|(name, bytes)| {
            let path = storage.join(format!("817c89b9_{name}"));
            std::fs::write(&path, bytes).unwrap();
            ContextImage {
                filename: (*name).to_string(),
                path: Some(path.to_string_lossy().to_string()),
            }
        })
        .collect();
    ContextImages {
        items,
        catalog_vision_models: HashSet::new(),
        language: "en".into(),
    }
}

fn with_vision_model(mut images: ContextImages, model: &str) -> ContextImages {
    images.catalog_vision_models.insert(model.to_string());
    images
}

#[test]
fn an_image_is_recognised_by_its_bytes_not_its_name() {
    assert_eq!(sniff_image_mime(&png_bytes(4)), Some("image/png"));
    assert_eq!(
        sniff_image_mime(&[0xff, 0xd8, 0xff, 0xe0, 0, 0x10]),
        Some("image/jpeg")
    );
    assert_eq!(sniff_image_mime(b"GIF89a\x01\x00"), Some("image/gif"));
    assert_eq!(sniff_image_mime(b"GIF87a\x01\x00"), Some("image/gif"));
    assert_eq!(
        sniff_image_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
        Some("image/webp")
    );
    // A wav is RIFF too; an SVG and a text file are not pictures a model reads.
    assert_eq!(sniff_image_mime(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
    assert_eq!(
        sniff_image_mime(b"<svg xmlns='http://www.w3.org/2000/svg'/>"),
        None
    );
    assert_eq!(sniff_image_mime(b"just text"), None);
    assert_eq!(sniff_image_mime(&[]), None);
}

#[test]
fn ollama_reports_vision_in_capabilities_and_silence_proves_nothing() {
    assert_eq!(
        ollama_show_vision(&json!({"capabilities": ["completion", "vision", "tools"]})),
        ImageSupport::Supported
    );
    assert_eq!(
        ollama_show_vision(&json!({"capabilities": ["completion"]})),
        ImageSupport::Unsupported
    );
    // An older server answers without the field.
    assert_eq!(
        ollama_show_vision(&json!({"details": {"format": "gguf"}})),
        ImageSupport::Unknown
    );
}

#[test]
fn litellm_declares_vision_per_deployment_and_an_absent_flag_is_unknown() {
    let info = json!({"data": [
        {"model_name": "gemini-3.6-flash", "model_info": {"supports_vision": true}},
        {"model_name": "text-only", "model_info": {"supports_vision": false}},
        {"model_name": "unmapped", "model_info": {}},
        {"model_name": "no-info"},
    ]});
    assert_eq!(
        litellm_model_info_vision(&info, "gemini-3.6-flash"),
        ImageSupport::Supported
    );
    // `false` is LiteLLM's default for a model it has no entry for: "not
    // declared", never a verdict.
    for alias in ["text-only", "unmapped", "no-info", "not-listed"] {
        assert_eq!(
            litellm_model_info_vision(&info, alias),
            ImageSupport::Unknown,
            "{alias}"
        );
    }
    assert_eq!(
        litellm_model_info_vision(&json!({}), "gemini-3.6-flash"),
        ImageSupport::Unknown
    );
}

#[tokio::test]
async fn a_model_the_catalogue_says_can_see_gets_the_image_itself() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = png_bytes(64);
    let images = with_vision_model(
        attached(&dir, &[("image.png", bytes.clone())]),
        "gemini-3.6-flash",
    );
    // The provider knows nothing: the operator's catalogue word is enough.
    let prepared = prepare(&images, "gemini-3.6-flash", ImageSupport::Unknown).await;

    assert!(prepared.withheld.is_empty());
    assert_eq!(prepared.sent.len(), 1);
    assert_eq!(prepared.sent[0].mime, "image/png");
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(&prepared.sent[0].base64)
            .unwrap(),
        bytes,
        "the bytes on the wire are the attached bytes"
    );
    let notice = prepared.notice();
    assert!(notice.contains("`image.png`") && notice.contains("you can see it"));
    assert!(
        !notice.contains("context-files"),
        "an image the model sees is not also handed over as a path: {notice}"
    );
}

#[tokio::test]
async fn the_provider_can_vouch_for_a_model_the_catalogue_is_silent_about() {
    let dir = tempfile::tempdir().unwrap();
    let images = attached(&dir, &[("shot.png", png_bytes(8))]);
    let prepared = prepare(&images, "llama-vision", ImageSupport::Supported).await;
    assert_eq!(prepared.sent.len(), 1);
    assert_eq!(prepared.support_source, "provider");
}

#[tokio::test]
async fn a_model_that_cannot_see_is_told_so_and_never_handed_a_path() {
    let dir = tempfile::tempdir().unwrap();
    let images = attached(&dir, &[("image.png", png_bytes(8))]);
    let path = images.items[0].path.clone().unwrap();

    // Positively text-only, and "nobody knows": the same refusal to guess.
    for provider in [ImageSupport::Unsupported, ImageSupport::Unknown] {
        let prepared = prepare(&images, "text-model", provider).await;
        assert!(prepared.sent.is_empty(), "{provider:?}");
        let notice = prepared.notice();
        assert!(
            notice.contains(
                "An image `image.png` is attached, but this model cannot see it: do not \
                 describe its content, ask the user to describe it or to switch to a model \
                 that accepts images."
            ),
            "{provider:?}: {notice}"
        );
        assert!(
            !notice.contains(&path) && !notice.contains("context-files"),
            "a path the model cannot open is what invited the invention: {notice}"
        );
    }
}

#[tokio::test]
async fn the_refusal_speaks_french_in_a_french_room() {
    let dir = tempfile::tempdir().unwrap();
    let mut images = attached(&dir, &[("image.png", png_bytes(8))]);
    images.language = "fr".into();
    let prepared = prepare(&images, "text-model", ImageSupport::Unknown).await;
    assert!(
        prepared.notice().contains(
            "Une image `image.png` est jointe, mais ce modèle ne peut pas la voir : ne décris \
             pas son contenu, demande à l'utilisateur de la décrire ou de changer de modèle."
        ),
        "{}",
        prepared.notice()
    );
}

#[tokio::test]
async fn the_catalogue_beats_a_provider_that_says_no() {
    let dir = tempfile::tempdir().unwrap();
    let images = with_vision_model(attached(&dir, &[("a.png", png_bytes(8))]), "tuned");
    let prepared = prepare(&images, "tuned", ImageSupport::Unsupported).await;
    assert_eq!(prepared.sent.len(), 1);
    assert_eq!(prepared.support_source, "catalogue");
}

#[tokio::test]
async fn an_image_that_cannot_be_sent_is_accounted_for_not_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let images = with_vision_model(
        attached(
            &dir,
            &[
                ("huge.png", png_bytes(MAX_IMAGE_BYTES)),
                (
                    "logo.svg",
                    b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(),
                ),
                ("fine.png", png_bytes(16)),
            ],
        ),
        "m",
    );
    let mut images = images;
    images.items.push(ContextImage {
        filename: "legacy.png".into(),
        path: None,
    });
    images.items.push(ContextImage {
        filename: "gone.png".into(),
        path: Some(
            dir.path()
                .join("context-files/missing.png")
                .to_string_lossy()
                .to_string(),
        ),
    });
    let prepared = prepare(&images, "m", ImageSupport::Unknown).await;

    assert_eq!(
        prepared
            .sent
            .iter()
            .map(|i| i.filename.as_str())
            .collect::<Vec<_>>(),
        ["fine.png"]
    );
    let reasons: std::collections::HashMap<_, _> = prepared
        .withheld
        .iter()
        .map(|(name, reason)| (name.as_str(), reason.clone()))
        .collect();
    assert!(matches!(reasons["huge.png"], Withheld::TooLarge { .. }));
    assert_eq!(reasons["logo.svg"], Withheld::UnsupportedFormat);
    assert_eq!(reasons["legacy.png"], Withheld::Unreadable);
    assert_eq!(reasons["gone.png"], Withheld::Unreadable);

    let notice = prepared.notice();
    assert!(notice.contains("`huge.png`") && notice.contains("exceeds the size that can be sent"));
    assert!(notice.contains("`logo.svg`") && notice.contains("only PNG, JPEG, GIF and WebP"));
    assert!(notice.contains("`legacy.png`") && notice.contains("cannot be read"));
    // Every unseen image carries the instruction not to describe it.
    assert_eq!(
        notice.matches("do not describe its content").count(),
        4,
        "{notice}"
    );
}

#[tokio::test]
async fn only_the_most_recent_images_are_sent_and_the_rest_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let names: Vec<String> = (0..MAX_IMAGES + 2)
        .map(|n| format!("shot{n}.png"))
        .collect();
    let files: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), png_bytes(8))).collect();
    let images = with_vision_model(attached(&dir, &files), "m");
    let prepared = prepare(&images, "m", ImageSupport::Unknown).await;

    assert_eq!(prepared.sent.len(), MAX_IMAGES);
    assert_eq!(
        prepared.sent[0].filename, "shot2.png",
        "the oldest two are cut"
    );
    assert_eq!(
        prepared.withheld,
        vec![
            ("shot0.png".to_string(), Withheld::TooMany),
            ("shot1.png".to_string(), Withheld::TooMany)
        ]
    );
}

#[tokio::test]
async fn the_request_has_an_image_budget() {
    let dir = tempfile::tempdir().unwrap();
    // Three images under the per-image cap whose sum is over the request cap.
    let each = MAX_IMAGE_BYTES - 16;
    let images = with_vision_model(
        attached(
            &dir,
            &[
                ("a.png", png_bytes(each)),
                ("b.png", png_bytes(each)),
                ("c.png", png_bytes(each)),
            ],
        ),
        "m",
    );
    let prepared = prepare(&images, "m", ImageSupport::Unknown).await;
    assert_eq!(prepared.sent.len(), 2);
    assert_eq!(
        prepared.withheld,
        vec![("c.png".to_string(), Withheld::TotalBudget)]
    );
    assert!(prepared.sent.iter().map(|i| i.bytes).sum::<usize>() <= MAX_TOTAL_BYTES);
}

#[tokio::test]
async fn a_discussion_without_images_gets_no_notice_and_no_work() {
    let prepared = prepare(&ContextImages::default(), "m", ImageSupport::Supported).await;
    assert!(prepared.sent.is_empty() && prepared.withheld.is_empty());
    assert_eq!(prepared.notice(), "");
    assert_eq!(prepared.prompt_bytes(), 0);
}

#[tokio::test]
async fn a_large_valid_image_is_downscaled_without_rewriting_the_attachment() {
    let picture = image::RgbImage::from_pixel(2300, 1800, image::Rgb([70, 120, 180]));
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(picture)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    let mut bytes = output.into_inner();
    // Trailing bytes are allowed by the PNG decoder; make the size trigger deterministic.
    bytes.resize(bytes.len().max(MAX_IMAGE_BYTES + 1), 0);
    assert!(bytes.len() <= MAX_SOURCE_BYTES);
    let dir = tempfile::tempdir().unwrap();
    let images = with_vision_model(attached(&dir, &[("large.png", bytes.clone())]), "m");
    let original_path = images.items[0].path.as_ref().unwrap();
    let prepared = prepare(&images, "m", ImageSupport::Unknown).await;
    assert!(prepared.withheld.is_empty());
    assert_eq!(prepared.sent.len(), 1);
    assert!(prepared.sent[0].bytes <= MAX_IMAGE_BYTES);
    let sent = base64::engine::general_purpose::STANDARD
        .decode(&prepared.sent[0].base64)
        .unwrap();
    let resized = image::load_from_memory(&sent).unwrap();
    assert!(resized.width() <= 2048 && resized.height() <= 2048);
    assert_eq!(std::fs::read(original_path).unwrap(), bytes);
}

fn sent(name: &str) -> SentImage {
    SentImage {
        filename: name.into(),
        mime: "image/png",
        base64: "QUJD".into(),
        bytes: 3,
    }
}

fn base_body() -> Value {
    json!({
        "model": "m",
        "messages": [
            {"role": "system", "content": "context"},
            {"role": "user", "content": "what is in the picture?"},
        ],
    })
}

#[test]
fn an_openai_compatible_body_carries_the_image_as_a_data_url_part() {
    let mut body = base_body();
    attach_to_chat_body(&mut body, true, &[sent("a.png"), sent("b.png")]);

    assert_eq!(
        body["messages"][0]["content"], "context",
        "system untouched"
    );
    let parts = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(
        parts[0],
        json!({"type": "text", "text": "what is in the picture?"})
    );
    for part in &parts[1..] {
        assert_eq!(part["type"], "image_url");
        assert_eq!(part["image_url"]["url"], "data:image/png;base64,QUJD");
    }
    assert!(body["messages"][1].get("images").is_none());
}

#[test]
fn an_ollama_body_carries_the_image_in_the_images_array() {
    let mut body = base_body();
    attach_to_chat_body(&mut body, false, &[sent("a.png")]);

    assert_eq!(
        body["messages"][1]["content"], "what is in the picture?",
        "Ollama keeps the text as a plain string"
    );
    assert_eq!(body["messages"][1]["images"], json!(["QUJD"]));
    assert!(body["messages"][0].get("images").is_none());
}

#[test]
fn images_go_to_the_last_user_message_only_and_nothing_to_attach_changes_nothing() {
    let mut body = json!({"messages": [
        {"role": "user", "content": "first"},
        {"role": "assistant", "content": "ok"},
        {"role": "user", "content": "second"},
    ]});
    attach_to_chat_body(&mut body, true, &[sent("a.png")]);
    assert_eq!(body["messages"][0]["content"], "first");
    assert!(body["messages"][2]["content"].is_array());

    let mut untouched = base_body();
    attach_to_chat_body(&mut untouched, true, &[]);
    assert_eq!(untouched, base_body());
}

#[test]
fn a_screenshot_is_not_priced_by_its_base64() {
    let mut body = base_body();
    let huge = SentImage {
        base64: "A".repeat(4_000_000),
        bytes: 3_000_000,
        ..sent("big.png")
    };
    let text_only = messages_wire_len(&body);
    attach_to_chat_body(&mut body, true, std::slice::from_ref(&huge));
    let with_image = messages_wire_len(&body);
    assert!(
        with_image < text_only + IMAGE_PROMPT_BYTES + 200,
        "one image adds a flat estimate, not its payload: {text_only} -> {with_image}"
    );
    assert!(with_image >= text_only + IMAGE_PROMPT_BYTES - 200);

    let mut ollama = base_body();
    attach_to_chat_body(&mut ollama, false, &[huge]);
    assert!(messages_wire_len(&ollama) < text_only + IMAGE_PROMPT_BYTES + 200);
}

#[test]
fn a_notice_is_empty_when_nothing_is_attached() {
    assert_eq!(PreparedImages::default().notice(), "");
}
