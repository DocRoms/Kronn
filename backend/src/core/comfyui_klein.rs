use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::comfyui::{size_for, ResolvedGenSettings, IMAGE_SIZES};
use crate::models::MediaParams;

pub const TEXT_ENCODER: &str = "qwen_3_4b.safetensors";
pub const VAE: &str = "flux2-vae.safetensors";
pub const REFERENCE_LIMIT: u32 = 4;
pub const REQUIRED_NODES: &[&str] = &[
    "UNETLoader",
    "CLIPLoader",
    "VAELoader",
    "CLIPTextEncode",
    "ConditioningZeroOut",
    "LoadImage",
    "ImageScaleToTotalPixels",
    "VAEEncode",
    "ReferenceLatent",
    "EmptyFlux2LatentImage",
    "Flux2Scheduler",
    "CFGGuider",
    "RandomNoise",
    "KSamplerSelect",
    "SamplerCustomAdvanced",
    "VAEDecode",
    "SaveImage",
];

/// Comfy-Org's Klein distilled edit recipe, extended by chaining one latent
/// per reference: https://docs.comfy.org/tutorials/flux/flux-2-klein
pub fn workflow(
    model: &str,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
    settings: &ResolvedGenSettings,
    images: &[String],
) -> Result<Value> {
    if images.len() > REFERENCE_LIMIT as usize {
        bail!("FLUX.2 Klein accepts at most {REFERENCE_LIMIT} source images");
    }
    let (width, height) = size_for(IMAGE_SIZES, params.aspect_ratio.as_deref())?;
    let mut graph = json!({
        "model": {"class_type":"UNETLoader","inputs":{"unet_name":model,"weight_dtype":"default"}},
        "clip": {"class_type":"CLIPLoader","inputs":{"clip_name":TEXT_ENCODER,"type":"flux2","device":"default"}},
        "vae": {"class_type":"VAELoader","inputs":{"vae_name":VAE}},
        "positive": {"class_type":"CLIPTextEncode","inputs":{"clip":["clip",0],"text":prompt}},
        "negative": {"class_type":"ConditioningZeroOut","inputs":{"conditioning":["positive",0]}},
        "latent": {"class_type":"EmptyFlux2LatentImage","inputs":{"width":width,"height":height,"batch_size":1}},
        "sigmas": {"class_type":"Flux2Scheduler","inputs":{"steps":settings.steps,"width":width,"height":height}},
        "noise": {"class_type":"RandomNoise","inputs":{"noise_seed":seed}},
        "sampler": {"class_type":"KSamplerSelect","inputs":{"sampler_name":settings.sampler}},
        "guider": {"class_type":"CFGGuider","inputs":{"model":["model",0],"positive":["positive",0],"negative":["negative",0],"cfg":settings.cfg}},
        "sample": {"class_type":"SamplerCustomAdvanced","inputs":{"noise":["noise",0],"guider":["guider",0],"sampler":["sampler",0],"sigmas":["sigmas",0],"latent_image":["latent",0]}},
        "decode": {"class_type":"VAEDecode","inputs":{"samples":["sample",0],"vae":["vae",0]}},
        "save": {"class_type":"SaveImage","inputs":{"images":["decode",0],"filename_prefix":"kronn/image"}}
    });
    if settings.scheduler != "flux2" {
        graph["sigmas"] = json!({"class_type":"BasicScheduler","inputs":{
            "model":["model",0],"scheduler":settings.scheduler,"steps":settings.steps,"denoise":1.0}});
    }
    for (i, filename) in images.iter().enumerate() {
        let load = format!("reference_load_{i}");
        let scale = format!("reference_scale_{i}");
        let encode = format!("reference_encode_{i}");
        graph[&load] = json!({"class_type":"LoadImage","inputs":{"image":filename}});
        graph[&scale] = json!({"class_type":"ImageScaleToTotalPixels","inputs":{
            "image":[load,0],"upscale_method":"lanczos","megapixels":1.0,"resolution_steps":1}});
        graph[&encode] =
            json!({"class_type":"VAEEncode","inputs":{"pixels":[scale,0],"vae":["vae",0]}});
        for side in ["positive", "negative"] {
            let apply = format!("reference_{side}_{i}");
            let previous = graph["guider"]["inputs"][side].clone();
            graph[&apply] = json!({"class_type":"ReferenceLatent","inputs":{
                "conditioning":previous,"latent":[encode,0]}});
            graph["guider"]["inputs"][side] = json!([apply, 0]);
        }
    }
    Ok(graph)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::core::comfyui::ComfyFamily;

    #[test]
    fn klein_graph_matches_the_installed_comfyui_node_inputs() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/comfyui-klein-nodes-0.42.1.json"
        ))
        .unwrap();
        for count in [0, 3, 4] {
            for scheduler in ["flux2", "simple"] {
                let settings = ResolvedGenSettings {
                    scheduler: scheduler.into(),
                    ..ComfyFamily::Flux2Klein4b.defaults()
                };
                let graph = workflow(
                    "flux-2-klein-4b.safetensors",
                    "compose",
                    &MediaParams::default(),
                    1,
                    &settings,
                    &vec!["source.png".into(); count],
                )
                .unwrap();
                for (id, node) in graph.as_object().unwrap() {
                    let schema = &contract["nodes"][node["class_type"].as_str().unwrap()];
                    let required = schema["required"].as_array().expect("captured node schema");
                    let optional = schema["optional"].as_array().unwrap();
                    let inputs = node["inputs"].as_object().unwrap();
                    for field in required {
                        assert!(
                            inputs.contains_key(field.as_str().unwrap()),
                            "{id} missing required input {field}"
                        );
                    }
                    for (field, value) in inputs {
                        assert!(
                            required
                                .iter()
                                .chain(optional)
                                .any(|name| name.as_str() == Some(field)),
                            "{id} has unknown input {field}"
                        );
                        if let Some(link) = value.as_array() {
                            let source = &graph[link[0].as_str().unwrap()];
                            let outputs = contract["nodes"][source["class_type"].as_str().unwrap()]
                                ["outputs"]
                                .as_array()
                                .unwrap();
                            assert!(
                                (link[1].as_u64().unwrap() as usize) < outputs.len(),
                                "{id}.{field} references a missing output"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn klein_conditions_on_every_reference_in_order_and_keeps_text_only_support() {
        for count in 0..=4 {
            let images: Vec<_> = (0..count).map(|i| format!("référence-{i}.png")).collect();
            let graph = workflow(
                "klein",
                "compose",
                &MediaParams::default(),
                17,
                &ComfyFamily::Flux2Klein4b.defaults(),
                &images,
            )
            .unwrap();
            assert_eq!(graph["model"]["inputs"]["unet_name"], "klein");
            assert_eq!(graph["clip"]["inputs"]["type"], "flux2");
            assert_eq!(graph["sigmas"]["inputs"]["steps"], 4);
            assert_eq!(graph["noise"]["inputs"]["noise_seed"], 17);
            for side in ["positive", "negative"] {
                let mut link = graph["guider"]["inputs"][side].clone();
                for i in (0..count).rev() {
                    assert_eq!(link, json!([format!("reference_{side}_{i}"), 0]));
                    let node = &graph[link[0].as_str().unwrap()];
                    assert_eq!(
                        node["inputs"]["latent"],
                        json!([format!("reference_encode_{i}"), 0])
                    );
                    link = node["inputs"]["conditioning"].clone();
                }
                assert_eq!(link, json!([side, 0]));
            }
            for (i, filename) in images.iter().enumerate() {
                assert_eq!(
                    graph[format!("reference_load_{i}")]["inputs"]["image"],
                    *filename
                );
                assert_eq!(
                    graph[format!("reference_encode_{i}")]["inputs"]["pixels"],
                    json!([format!("reference_scale_{i}"), 0])
                );
                assert_eq!(
                    graph[format!("reference_scale_{i}")]["inputs"]["image"],
                    json!([format!("reference_load_{i}"), 0])
                );
            }
        }
        assert!(workflow(
            "klein",
            "p",
            &MediaParams::default(),
            0,
            &ComfyFamily::Flux2Klein4b.defaults(),
            &vec!["x".into(); 5]
        )
        .is_err());
    }

    #[test]
    fn klein_respects_sampler_settings_and_output_ratio() {
        let settings = ResolvedGenSettings {
            steps: 8,
            cfg: 2.0,
            sampler: "heun".into(),
            scheduler: "simple".into(),
        };
        let params = MediaParams {
            aspect_ratio: Some("16:9".into()),
            ..Default::default()
        };
        let graph = workflow("klein", "p", &params, 42, &settings, &[]).unwrap();
        assert_eq!(graph["latent"]["inputs"]["width"], 1344);
        assert_eq!(graph["latent"]["inputs"]["height"], 768);
        assert_eq!(graph["sigmas"]["class_type"], "BasicScheduler");
        assert_eq!(graph["sigmas"]["inputs"]["steps"], 8);
        assert_eq!(graph["sampler"]["inputs"]["sampler_name"], "heun");
        assert_eq!(graph["guider"]["inputs"]["cfg"], 2.0);
    }
}
