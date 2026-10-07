//! KT-946 — what actually leaves the machine when a discussion has an attached
//! image: through the real HTTP start path, against a mock provider, on both wire
//! formats. The reported failure was a request with no image in it at all.

use super::*;
use crate::agents::vision::{ContextImage, ContextImages};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG_B64: &str = "iVBORw0KGgo="; // the 8-byte PNG signature, base64

fn png_signature() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n".to_vec()
}

/// The discussion's attachments, stored the way Kronn stores them.
fn one_image(dir: &tempfile::TempDir, bytes: Vec<u8>) -> (ContextImages, String) {
    let storage = dir.path().join("context-files");
    std::fs::create_dir_all(&storage).unwrap();
    let file = storage.join("817c89b9_image.png");
    std::fs::write(&file, bytes).unwrap();
    let path = file.to_string_lossy().to_string();
    (
        ContextImages {
            items: vec![ContextImage {
                filename: "image.png".into(),
                path: Some(path.clone()),
            }],
            catalog_vision_models: Default::default(),
            language: "en".into(),
        },
        path,
    )
}

async fn mount_openai_reply(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n",
        ))
        .mount(server)
        .await;
}

async fn mount_ollama_reply(server: &MockServer, capabilities: Option<&str>) {
    let show = match capabilities {
        Some(list) => format!(
            r#"{{"details":{{"format":"gguf"}},"model_info":{{"test.context_length":32768}},"capabilities":{list}}}"#
        ),
        None => {
            r#"{"details":{"format":"gguf"},"model_info":{"test.context_length":32768}}"#.into()
        }
    };
    Mock::given(method("POST"))
        .and(path("/api/show"))
        .respond_with(ResponseTemplate::new(200).set_body_string(show))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "{\"message\":{\"content\":\"ok\"},\"done\":false}\n{\"done\":true,\"prompt_eval_count\":5,\"eval_count\":2}\n",
        ))
        .mount(server)
        .await;
}

async fn run(
    agent: &AgentType,
    server: &MockServer,
    model: &str,
    images: &ContextImages,
) -> serde_json::Value {
    let mut process = start_ollama_http_with_idle(
        agent,
        "what is in the picture?",
        "=== CONTEXT FILES ===\n\nnotes",
        model,
        None,
        Some(&server.uri()),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(images),
        None,
    )
    .await
    .expect("the run starts");
    while process.next_line().await.is_some() {}
    let chat = if is_openai_wire_agent(agent) {
        "/v1/chat/completions"
    } else {
        "/api/chat"
    };
    let requests = server.received_requests().await.expect("recorded");
    let request = requests
        .iter()
        .find(|request| request.url.path() == chat)
        .expect("the model was called");
    serde_json::from_slice(&request.body).expect("a JSON chat body")
}

fn system_text(body: &serde_json::Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn last_user(body: &serde_json::Value) -> &serde_json::Value {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .unwrap()
}

#[tokio::test]
async fn litellm_vision_model_receives_the_image_as_an_image_url_part() {
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    Mock::given(method("GET"))
        .and(path("/model/info"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"data":[{"model_name":"gemini-3.6-flash","model_info":{"supports_vision":true}}]}"#,
        ))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let (images, path) = one_image(&dir, png_signature());

    let body = run(&AgentType::LiteLlm, &server, "gemini-3.6-flash", &images).await;

    let parts = last_user(&body)["content"]
        .as_array()
        .expect("the user content is a list of parts once an image rides along");
    assert_eq!(parts[0]["type"], "text");
    assert_eq!(parts[0]["text"], "what is in the picture?");
    assert_eq!(parts[1]["type"], "image_url");
    assert_eq!(
        parts[1]["image_url"]["url"],
        format!("data:image/png;base64,{PNG_B64}"),
        "the attached bytes, not a path"
    );
    let system = system_text(&body);
    assert!(system.contains("=== ATTACHED IMAGES ==="), "{system}");
    assert!(system.contains("`image.png`") && system.contains("you can see it"));
    assert!(
        !body.to_string().contains(&path),
        "the storage path must not reach the model"
    );
}

#[tokio::test]
async fn litellm_model_that_cannot_see_is_told_so_and_gets_no_image_part() {
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    // No /model/info route: the capability is simply unknown.
    let dir = tempfile::tempdir().unwrap();
    let (images, path) = one_image(&dir, png_signature());

    let body = run(&AgentType::LiteLlm, &server, "mystery-model", &images).await;

    assert_eq!(
        last_user(&body)["content"],
        "what is in the picture?",
        "an unknown capability sends no image"
    );
    assert!(!body.to_string().contains("image_url"));
    let system = system_text(&body);
    assert!(
        system.contains(
            "An image `image.png` is attached, but this model cannot see it: do not describe \
             its content, ask the user to describe it or to switch to a model that accepts images."
        ),
        "{system}"
    );
    assert!(
        !body.to_string().contains(&path),
        "no path without the mention"
    );
}

#[tokio::test]
async fn nvidia_and_a_named_connection_follow_the_same_rule() {
    // NVIDIA exposes no capability route: only the catalogue can vouch.
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let (mut images, _) = one_image(&dir, png_signature());

    let body = run(&AgentType::Nvidia, &server, "some/vlm", &images).await;
    assert!(!body.to_string().contains("image_url"));
    assert!(system_text(&body).contains("cannot see it"));

    images.catalog_vision_models.insert("some/vlm".into());
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    let body = run(&AgentType::Nvidia, &server, "some/vlm", &images).await;
    assert_eq!(last_user(&body)["content"][1]["type"], "image_url");

    // A named external connection, whose endpoint is the mock itself.
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    let body = run(&AgentType::Custom, &server, "some/vlm", &images).await;
    assert_eq!(last_user(&body)["content"][1]["type"], "image_url");
}

#[tokio::test]
async fn ollama_vision_model_receives_the_image_in_the_images_array() {
    let server = MockServer::start().await;
    mount_ollama_reply(&server, Some(r#"["completion","vision"]"#)).await;
    let dir = tempfile::tempdir().unwrap();
    let (images, path) = one_image(&dir, png_signature());

    let body = run(&AgentType::Ollama, &server, "gemma3:12b", &images).await;

    let user = last_user(&body);
    assert_eq!(user["content"], "what is in the picture?");
    assert_eq!(user["images"], serde_json::json!([PNG_B64]));
    let system = system_text(&body);
    assert!(system.contains("you can see it"), "{system}");
    assert!(!body.to_string().contains(&path));
}

#[tokio::test]
async fn ollama_text_model_is_told_it_cannot_see_the_image() {
    for capabilities in [Some(r#"["completion"]"#), None] {
        let server = MockServer::start().await;
        mount_ollama_reply(&server, capabilities).await;
        let dir = tempfile::tempdir().unwrap();
        let (images, path) = one_image(&dir, png_signature());

        let body = run(&AgentType::Ollama, &server, "qwen3:32b", &images).await;

        assert!(
            last_user(&body).get("images").is_none(),
            "{capabilities:?}: no image for a model that cannot take one"
        );
        assert!(
            system_text(&body).contains("this model cannot see it: do not describe its content"),
            "{capabilities:?}"
        );
        assert!(!body.to_string().contains(&path));
    }
}

#[tokio::test]
async fn a_large_screenshot_does_not_trip_the_context_guard() {
    // 3 MB of payload is ~4 MB of base64: priced by its bytes it would look like
    // a million tokens and the run would be refused as oversized before sending.
    let server = MockServer::start().await;
    mount_ollama_reply(&server, Some(r#"["vision"]"#)).await;
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = png_signature();
    bytes.extend(std::iter::repeat_n(1u8, 3 * 1024 * 1024));
    let (images, _) = one_image(&dir, bytes);

    let body = run(&AgentType::Ollama, &server, "gemma3:12b", &images).await;

    assert_eq!(last_user(&body)["images"].as_array().unwrap().len(), 1);
    let num_ctx = body["options"]["num_ctx"].as_u64().unwrap();
    assert!(
        num_ctx <= 32768,
        "the window stays within the model's own: {num_ctx}"
    );
}

#[tokio::test]
async fn no_images_means_no_notice_and_a_plain_request() {
    let server = MockServer::start().await;
    mount_openai_reply(&server).await;
    let body = run(
        &AgentType::LiteLlm,
        &server,
        "any-model",
        &ContextImages::default(),
    )
    .await;
    assert_eq!(last_user(&body)["content"], "what is in the picture?");
    assert!(!system_text(&body).contains("ATTACHED IMAGES"));
}

// ── Socket-free coverage of the same glue ───────────────────────────────────
//
// The tests above drive the real start path against a mock provider, which needs
// a local port. These build the very same requests through the production
// builders without one, so the wire shapes and the size accounting stay checked
// where binding a port is not allowed.

fn prepared_for(
    images: &ContextImages,
    model: &str,
    provider: crate::agents::vision::ImageSupport,
) -> crate::agents::vision::PreparedImages {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(crate::agents::vision::prepare(images, model, provider))
}

fn system_with_notice(prepared: &crate::agents::vision::PreparedImages) -> String {
    format!("=== CONTEXT FILES ===\n\nnotes\n\n{}", prepared.notice())
}

#[test]
fn the_openai_request_built_for_a_vision_model_carries_the_image_and_no_path() {
    let dir = tempfile::tempdir().unwrap();
    let (images, path) = one_image(&dir, png_signature());
    let prepared = prepared_for(
        &images,
        "gemini-3.6-flash",
        crate::agents::vision::ImageSupport::Supported,
    );
    let mut body = crate::agents::chat_codec::build_openai_chat_body(
        "gemini-3.6-flash",
        &system_with_notice(&prepared),
        "what is in the picture?",
        None,
        true,
    );
    crate::agents::vision::attach_to_chat_body(&mut body, true, &prepared.sent);

    let parts = last_user(&body)["content"].as_array().unwrap();
    assert_eq!(parts[0]["text"], "what is in the picture?");
    assert_eq!(
        parts[1]["image_url"]["url"],
        format!("data:image/png;base64,{PNG_B64}")
    );
    assert!(system_text(&body).contains("you can see it"));
    assert!(!body.to_string().contains(&path));
}

#[test]
fn the_openai_request_built_for_a_blind_model_has_no_image_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let (images, path) = one_image(&dir, png_signature());
    for provider in [
        crate::agents::vision::ImageSupport::Unsupported,
        crate::agents::vision::ImageSupport::Unknown,
    ] {
        let prepared = prepared_for(&images, "mystery-model", provider);
        let mut body = crate::agents::chat_codec::build_openai_chat_body(
            "mystery-model",
            &system_with_notice(&prepared),
            "what is in the picture?",
            None,
            true,
        );
        crate::agents::vision::attach_to_chat_body(&mut body, true, &prepared.sent);

        assert_eq!(last_user(&body)["content"], "what is in the picture?");
        assert!(!body.to_string().contains("image_url"));
        assert!(
            system_text(&body).contains("this model cannot see it: do not describe its content")
        );
        assert!(!body.to_string().contains(&path), "{provider:?}");
    }
}

#[test]
fn the_ollama_request_carries_the_image_and_a_screenshot_does_not_inflate_the_window() {
    let dir = tempfile::tempdir().unwrap();
    // ~3 MB: priced by its base64 this would be ~1.4 million tokens.
    let mut bytes = png_signature();
    bytes.extend(std::iter::repeat_n(1u8, 3 * 1024 * 1024));
    let (images, path) = one_image(&dir, bytes);
    let prepared = prepared_for(
        &images,
        "gemma3:12b",
        crate::agents::vision::ImageSupport::Supported,
    );
    assert_eq!(prepared.sent.len(), 1);
    let system = system_with_notice(&prepared);
    let mut body = build_ollama_chat_body(
        "gemma3:12b",
        &system,
        "what is in the picture?",
        None,
        32768,
        None,
    );
    crate::agents::vision::attach_to_chat_body(&mut body, false, &prepared.sent);

    let user = last_user(&body);
    assert_eq!(user["content"], "what is in the picture?");
    assert_eq!(user["images"].as_array().unwrap().len(), 1);
    assert!(!body["messages"].to_string().contains(&path));

    // The size guards read the body the way the tool loop does.
    fit_ollama_num_ctx(&mut body, 32768);
    assert!(
        body["options"]["num_ctx"].as_u64().unwrap() <= 8192,
        "an image is a flat few thousand tokens, not its payload: {}",
        body["options"]["num_ctx"]
    );
    assert!(estimated_chat_history_tokens(&body) < 8192);
}

#[tokio::test]
async fn capability_is_unknown_when_nobody_can_say_and_the_catalogue_short_circuits() {
    use crate::agents::vision::ImageSupport;
    let dir = tempfile::tempdir().unwrap();
    let (mut images, _) = one_image(&dir, png_signature());

    // NVIDIA exposes no capability route: with an empty catalogue it is unknown.
    assert_eq!(
        provider_image_support(&AgentType::Nvidia, &images, "some/vlm", None, None).await,
        ImageSupport::Unknown
    );
    // An unreachable proxy proves nothing either.
    assert_eq!(
        provider_image_support(
            &AgentType::LiteLlm,
            &images,
            "some/vlm",
            Some("http://127.0.0.1:1"),
            None
        )
        .await,
        ImageSupport::Unknown
    );
    // The catalogue's word wins before any probe is made, whatever the provider.
    images.catalog_vision_models.insert("some/vlm".into());
    for agent in [
        AgentType::Nvidia,
        AgentType::LiteLlm,
        AgentType::Custom,
        AgentType::Ollama,
    ] {
        assert_eq!(
            provider_image_support(
                &agent,
                &images,
                "some/vlm",
                Some("http://127.0.0.1:1"),
                None
            )
            .await,
            ImageSupport::Supported,
            "{agent:?}"
        );
    }
}
