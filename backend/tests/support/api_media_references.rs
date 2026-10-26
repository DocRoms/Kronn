use super::*;
use serde_json::json;
use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

/// Explicit local recipe only: creates a real unbilled generation on ComfyUI.
/// Supply KRONN_REFERENCE_RECIPE_DIR containing a.png, b.png and c.png, and
/// KRONN_REFERENCE_RECIPE_MODALITY=image|video. The proof stays in that directory.
#[tokio::test]
#[ignore = "requires an explicitly selected local ComfyUI with Redux or Wan VACE installed"]
async fn live_comfyui_three_reference_recipe() {
    use kronn::models::{ComfyUiConfig, MediaJobStatus};
    let directory = std::path::PathBuf::from(
        kronn::core::child_env::var("KRONN_REFERENCE_RECIPE_DIR").expect("recipe directory"),
    );
    let modality =
        kronn::core::child_env::var("KRONN_REFERENCE_RECIPE_MODALITY").expect("image or video");
    assert!(matches!(modality.as_str(), "image" | "video"));
    let endpoint = kronn::core::child_env::var("KRONN_REFERENCE_RECIPE_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:8188".into());
    let state = test_state();
    seed_media_connection(&state, None, None).await;
    let mut config = ComfyUiConfig {
        endpoint: Some(endpoint.clone()),
        image_model: Some("flux1-schnell-fp8.safetensors".into()),
        video_model: Some("wan2.1_vace_1.3B_fp16.safetensors".into()),
        ..Default::default()
    };
    config.video_settings.steps = Some(8);
    state.config.write().await.comfyui = config;
    for id in ["a", "b", "c"] {
        let file = directory.join(format!("{id}.png"));
        assert!(file.is_file());
        seed_reference_image(&state, id, "disc-media", "image/png", file.to_str()).await;
    }
    let app = build_router_with_auth(state.clone(), false);
    let mut request = json!({"connection_id":"local:comfyui", "modality":modality,
        "discussion_id":"disc-media", "prompt":"A clean white tabletop holding a red circle, a blue square and a green triangle from the reference images, side by side, slow smooth camera movement, all three objects visible.",
        "reference_mode":"reference", "reference_asset_ids":["a","b","c"], "aspect_ratio":"1:1"});
    if modality == "video" {
        request["duration_secs"] = json!(2);
    }
    std::fs::write(
        directory.join("request.json"),
        serde_json::to_vec_pretty(&request).unwrap(),
    )
    .unwrap();
    let (_, answer) = post_json(app.clone(), "/api/media/generate", request).await;
    assert_eq!(answer["success"], true, "{answer}");
    let id = answer["data"]["job_id"].as_str().unwrap().to_owned();
    println!("Recipe job: {id}");
    // Simulate changing the selected slot after queueing. The persisted job
    // must still use its accepted family, endpoint and sampler settings.
    state.config.write().await.comfyui = ComfyUiConfig::default();
    let client = reqwest::Client::new();
    for iteration in 0..1080 {
        kronn::agents::media_runner::tick(&state, &client)
            .await
            .unwrap();
        let lookup = id.clone();
        let job = state
            .db
            .with_read_conn(move |conn| kronn::db::media_jobs::get(conn, &lookup))
            .await
            .unwrap()
            .unwrap();
        if iteration % 12 == 0 {
            println!(
                "Recipe {}: {:?}, prompt {:?}, {:?}",
                id, job.status, job.provider_job_id, job.last_error
            );
        }
        if job.status.is_terminal() {
            assert_eq!(job.status, MediaJobStatus::Completed, "{job:?}");
            assert_eq!(job.params.reference_ids(), ["a", "b", "c"]);
            assert!(job.cost.is_none());
            let asset_id = job.context_file_id.clone().unwrap();
            let asset = state
                .db
                .with_read_conn(move |conn| {
                    Ok(kronn::db::discussions::get_context_file(conn, &asset_id)?)
                })
                .await
                .unwrap()
                .unwrap();
            let suffix = if modality == "image" { "png" } else { "mp4" };
            std::fs::copy(
                asset.disk_path.unwrap(),
                directory.join(format!("result.{suffix}")),
            )
            .unwrap();
            let (_, receipt) = get_json(app, &format!("/api/media/jobs/{id}")).await;
            std::fs::write(
                directory.join("receipt.json"),
                serde_json::to_vec_pretty(&receipt).unwrap(),
            )
            .unwrap();
            let handle = job.provider_job_id.unwrap();
            let history: Value = client
                .get(format!("{endpoint}/history/{handle}"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            let graph = &history[&handle]["prompt"][2];
            for index in 0..3 {
                assert!(graph[format!("reference_load_{index}")]["inputs"]["image"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("-{index}.png")));
            }
            std::fs::write(
                directory.join("history.json"),
                serde_json::to_vec_pretty(&history).unwrap(),
            )
            .unwrap();
            println!("Recipe completed: {}", directory.display());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    panic!("recipe exceeded the local generation deadline");
}

async fn remote() -> (AppState, Router, MockServer) {
    let state = test_state();
    seed_media_connection(&state, Some("image-model"), Some("video-model")).await;
    let server = MockServer::start().await;
    let base = server.uri();
    state
        .db
        .with_conn(move |conn| {
            conn.execute(
                "UPDATE external_api_connections SET endpoint = ?1 WHERE id = 'conn-media'",
                [base],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let app = build_router_with_auth(state.clone(), false);
    (state, app, server)
}

#[tokio::test]
async fn comfyui_three_sources_survive_a_slot_change_between_the_api_and_worker() {
    for modality in ["image", "video"] {
        let state = test_state();
        seed_media_connection(&state, None, None).await;
        let server = MockServer::start().await;
        for node in [
            "LoadImage",
            "StyleModelLoader",
            "CLIPVisionLoader",
            "CLIPVisionEncode",
            "StyleModelApply",
            "WanVaceToVideo",
            "TrimVideoLatent",
        ] {
            Mock::given(path(format!("/object_info/{node}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({node:{}})))
                .mount(&server)
                .await;
        }
        for (folder, files) in [
            ("checkpoints", vec!["flux1-schnell-fp8.safetensors"]),
            (
                "diffusion_models",
                vec!["wan2.1_vace_1.3B_fp16.safetensors"],
            ),
            ("style_models", vec!["flux1-redux-dev.safetensors"]),
            (
                "clip_vision",
                vec!["sigclip_vision_patch14_384.safetensors"],
            ),
            (
                "text_encoders",
                vec!["umt5_xxl_fp8_e4m3fn_scaled.safetensors"],
            ),
            ("vae", vec!["wan_2.1_vae.safetensors"]),
        ] {
            Mock::given(path(format!("/models/{folder}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(files))
                .mount(&server)
                .await;
        }
        Mock::given(path("/system_stats"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"system":{"comfyui_version":"0.42.1"}})),
            )
            .mount(&server)
            .await;
        Mock::given(path("/upload/image"))
            .respond_with(|request: &wiremock::Request| {
                let body = String::from_utf8_lossy(&request.body);
                let filename = body
                    .split("filename=\"")
                    .nth(1)
                    .unwrap()
                    .split('"')
                    .next()
                    .unwrap();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"name":filename,"subfolder":"","type":"input"}))
            })
            .expect(3)
            .mount(&server)
            .await;
        Mock::given(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"prompt_id":"three-reference-prompt"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let mut config = kronn::models::ComfyUiConfig {
            endpoint: Some(server.uri()),
            image_model: Some("flux1-schnell-fp8.safetensors".into()),
            video_model: Some("wan2.1_vace_1.3B_fp16.safetensors".into()),
            ..Default::default()
        };
        config.image_settings.steps = Some(7);
        config.video_settings.steps = Some(9);
        state.config.write().await.comfyui = config;
        let directory = tempfile::tempdir().unwrap();
        for id in ["a", "b", "c"] {
            let file = directory.path().join(id);
            std::fs::write(&file, format!("source-{id}")).unwrap();
            seed_reference_image(&state, id, "disc-media", "image/png", file.to_str()).await;
        }
        state
            .db
            .with_conn(|conn| {
                for (id, filename) in [
                    ("a", "rouge.png"),
                    ("b", "carré bleu.png"),
                    ("c", "vert.png"),
                ] {
                    conn.execute(
                        "UPDATE context_files SET filename = ?2 WHERE id = ?1",
                        rusqlite::params![id, filename],
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();
        let app = build_router_with_auth(state.clone(), false);
        let (_, answer) = post_json(app, "/api/media/generate", json!({
            "connection_id":"local:comfyui", "modality":modality, "discussion_id":"disc-media",
            "prompt":"compose", "reference_mode":"reference", "reference_asset_ids":["vert.png","a","carré bleu.png"]
        })).await;
        assert_eq!(answer["success"], true, "{answer}");
        state.config.write().await.comfyui = Default::default();
        kronn::agents::media_runner::tick(&state, &reqwest::Client::new())
            .await
            .unwrap();
        let id = answer["data"]["job_id"].as_str().unwrap().to_owned();
        let job = state
            .db
            .with_read_conn(move |conn| kronn::db::media_jobs::get(conn, &id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            job.provider_job_id.as_deref(),
            Some("three-reference-prompt"),
            "{job:?}"
        );
        assert_eq!(job.params.reference_ids(), ["c", "a", "b"]);
        let requests = server.received_requests().await.unwrap();
        let uploads: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path() == "/upload/image")
            .collect();
        for (index, source) in ["c", "a", "b"].iter().enumerate() {
            let body = String::from_utf8_lossy(&uploads[index].body);
            assert!(body.contains(&format!("source-{source}")));
            assert!(body.contains(&format!("-{index}.png")));
        }
        let submitted: Value = requests
            .iter()
            .find(|request| request.url.path() == "/prompt")
            .unwrap()
            .body_json()
            .unwrap();
        let graph = &submitted["prompt"];
        for index in 0..3 {
            assert!(graph[format!("reference_load_{index}")]["inputs"]["image"]
                .as_str()
                .unwrap()
                .ends_with(&format!("-{index}.png")));
        }
        let sampler = if modality == "image" { "5" } else { "8" };
        assert_eq!(
            graph[sampler]["inputs"]["steps"],
            if modality == "image" { 7 } else { 9 }
        );
    }
}

#[tokio::test]
async fn media_generate_refuses_a_stale_displayed_model_before_writing_a_job() {
    let (state, app, _server) = remote().await;
    let (_, refused) = post_json(
        app,
        "/api/media/generate",
        json!({
            "connection_id":"conn-media", "expected_model":"the-previous-model", "modality":"image",
            "discussion_id":"disc-media", "prompt":"compose"
        }),
    )
    .await;
    assert_eq!(refused["success"], false, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("model changed"));
    assert_eq!(
        state
            .db
            .with_read_conn(|conn| Ok(conn.query_row(
                "SELECT COUNT(*) FROM media_jobs",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn media_reference_limit_persists_zero_three_and_automatic_without_leaking_to_another_model()
{
    let (state, app, _server) = remote().await;
    let route = "/api/media/models?connection_id=conn-media&modality=image";
    let (_, initial) = get_json(app.clone(), route).await;
    assert!(
        initial["data"]["capabilities"]["max_input_references"].is_null(),
        "{initial}"
    );
    let request = |value: Value| json!({"connection_id":"conn-media", "model":"image-model", "modality":"image", "max_input_references":value});
    for maximum in [json!(3), json!(0), Value::Null] {
        let (_, saved) =
            put_json_root(app.clone(), "/api/media/models", request(maximum.clone())).await;
        assert_eq!(saved["success"], true, "{saved}");
        assert_eq!(
            saved["data"]["capabilities"]["max_input_references"],
            maximum
        );
        let persisted: toml::Value = toml::from_str(
            &std::fs::read_to_string(kronn::core::config::config_path().unwrap()).unwrap(),
        )
        .unwrap();
        let limits = persisted
            .get("media_reference_limits")
            .and_then(toml::Value::as_array);
        if maximum.is_null() {
            assert!(limits.is_none_or(Vec::is_empty));
        } else {
            assert_eq!(
                limits.unwrap()[0]["max_input_references"].as_integer(),
                maximum.as_i64()
            );
        }
    }
    let (_, saved) = put_json_root(app.clone(), "/api/media/models", request(json!(3))).await;
    assert_eq!(saved["success"], true);
    state.db.with_conn(|conn| {
        conn.execute("UPDATE external_api_connections SET image_model = 'changed' WHERE id = 'conn-media'", [])?;
        Ok(())
    }).await.unwrap();
    let (_, changed) = get_json(app.clone(), route).await;
    assert!(changed["data"]["capabilities"]["max_input_references"].is_null());
    let (_, refused) = put_json_root(app.clone(), "/api/media/models", request(json!(2))).await;
    assert_eq!(refused["success"], false);
    assert!(refused["error"].as_str().unwrap().contains("model changed"));
    let (_, invalid) = put_json_root(app, "/api/media/models", request(json!(65))).await;
    assert_eq!(invalid["success"], false);
}

#[tokio::test]
async fn media_reference_limit_cannot_exceed_the_provider_or_create_multi_reference_video_transport(
) {
    let (_state, app, server) = remote().await;
    Mock::given(path("/v1/images/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{
            "id":"image-model", "supported_parameters":{"input_references":{"type":"range","max":2}}
        }]})))
        .mount(&server)
        .await;
    let (_, refused) = put_json_root(app.clone(), "/api/media/models", json!({
        "connection_id":"conn-media", "model":"image-model", "modality":"image", "max_input_references":3
    })).await;
    assert_eq!(refused["success"], false, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("at most 2"));
    let (_, declared) = put_json_root(app, "/api/media/models", json!({
        "connection_id":"conn-media", "model":"video-model", "modality":"video", "max_input_references":3
    })).await;
    assert_eq!(declared["success"], true, "{declared}");
    assert_eq!(declared["data"]["capabilities"]["max_input_references"], 0);
    assert!(
        declared["data"]["capabilities"]["reference_support"]["blocker"]
            .as_str()
            .unwrap()
            .contains("input frames only")
    );
}

#[tokio::test]
async fn media_reference_preflight_rejects_missing_bytes_unknown_limits_and_excess_without_jobs() {
    let (state, app, _server) = remote().await;
    let directory = tempfile::tempdir().unwrap();
    for id in ["a", "b", "c"] {
        let file = directory.path().join(id);
        std::fs::write(&file, id).unwrap();
        seed_reference_image(&state, id, "disc-media", "image/png", file.to_str()).await;
    }
    seed_reference_image(
        &state,
        "gone",
        "disc-media",
        "image/png",
        directory.path().join("gone").to_str(),
    )
    .await;
    let request = |ids: Value| json!({"connection_id":"conn-media", "modality":"image", "discussion_id":"disc-media", "prompt":"compose", "reference_mode":"reference", "reference_asset_ids":ids});
    for (ids, expected) in [
        (json!(["gone"]), "no readable stored file"),
        (json!(["a"]), "unknown"),
    ] {
        let (_, refused) = post_json(app.clone(), "/api/media/generate", request(ids)).await;
        assert_eq!(refused["success"], false, "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains(expected),
            "{refused}"
        );
    }
    let (_, saved) = put_json_root(app.clone(), "/api/media/models", json!({"connection_id":"conn-media", "model":"image-model", "modality":"image", "max_input_references":2})).await;
    assert_eq!(saved["success"], true, "{saved}");
    let (_, excess) = post_json(
        app.clone(),
        "/api/media/generate",
        request(json!(["a", "b", "c"])),
    )
    .await;
    assert_eq!(excess["success"], false, "{excess}");
    assert!(excess["error"].as_str().unwrap().contains("at most 2"));
    assert_eq!(
        state
            .db
            .with_read_conn(|conn| Ok(conn.query_row(
                "SELECT COUNT(*) FROM media_jobs",
                [],
                |row| row.get::<_, i64>(0)
            )?))
            .await
            .unwrap(),
        0
    );
    let (_, saved) = put_json_root(app.clone(), "/api/media/models", json!({"connection_id":"conn-media", "model":"image-model", "modality":"image", "max_input_references":3})).await;
    assert_eq!(saved["success"], true);
    let (_, accepted) =
        post_json(app, "/api/media/generate", request(json!(["c", "a", "b"]))).await;
    assert_eq!(accepted["success"], true, "{accepted}");
    let id = accepted["data"]["job_id"].as_str().unwrap().to_owned();
    let job = state
        .db
        .with_read_conn(move |conn| kronn::db::media_jobs::get(conn, &id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.params.reference_ids(), ["c", "a", "b"]);
    assert_eq!(
        job.params.reference_mode,
        Some(kronn::models::MediaReferenceMode::Reference)
    );
}
