use anyhow::{bail, Result};
use serde_json::{json, Value};

use super::comfyui::{self, ComfyFamily, ResolvedGenSettings, WanCompanions};
use crate::models::MediaParams;

pub const REDUX_MODEL: &str = "flux1-redux-dev.safetensors";
pub const REDUX_VISION: &str = "sigclip_vision_patch14_384.safetensors";
pub const MULTI_REFERENCE_LIMIT: u32 = 3;

pub fn workflow_id(family: Option<ComfyFamily>) -> Option<&'static str> {
    match family {
        Some(ComfyFamily::Sdxl) => Some("sdxl-img2img-v1"),
        Some(ComfyFamily::FluxSchnell) => Some("flux-redux-v1"),
        Some(ComfyFamily::Flux2Klein4b) => Some("flux2-klein4b-references-v1"),
        Some(ComfyFamily::Wan21Vace) => Some("wan21-vace-references-v1"),
        _ => None,
    }
}

pub struct ImageReferences<'a> {
    pub images: &'a [String],
    pub denoise: f32,
}

pub fn image_workflow(
    family: ComfyFamily,
    checkpoint: &str,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
    settings: &ResolvedGenSettings,
    references: ImageReferences<'_>,
) -> Result<Value> {
    let ImageReferences { images, denoise } = references;
    if family == ComfyFamily::Flux2Klein4b {
        return super::comfyui_klein::workflow(checkpoint, prompt, params, seed, settings, images);
    }
    let mut graph = comfyui::image_workflow(family, checkpoint, prompt, params, seed, settings)?;
    if images.is_empty() {
        return Ok(graph);
    }
    match family {
        ComfyFamily::Sdxl => {
            if images.len() != 1 {
                bail!("Kronn's SDXL img2img workflow accepts one source; use FLUX Redux for multiple references");
            }
            if !denoise.is_finite() || !(0.0..1.0).contains(&denoise) {
                bail!("img2img denoise must be at least 0 and less than 1");
            }
            graph["reference_load"] =
                json!({"class_type":"LoadImage","inputs":{"image":images[0]}});
            graph["reference_encode"] = json!({"class_type":"VAEEncode","inputs":{
                "pixels":["reference_load",0],"vae":["1",2]}});
            graph["5"]["inputs"]["latent_image"] = json!(["reference_encode", 0]);
            graph["5"]["inputs"]["denoise"] = json!(denoise);
        }
        ComfyFamily::FluxSchnell => {
            check_count(images)?;
            graph["reference_style"] =
                json!({"class_type":"StyleModelLoader","inputs":{"style_model_name":REDUX_MODEL}});
            graph["reference_vision"] =
                json!({"class_type":"CLIPVisionLoader","inputs":{"clip_name":REDUX_VISION}});
            let mut conditioning = json!(["2", 0]);
            for (i, filename) in images.iter().enumerate() {
                let load = format!("reference_load_{i}");
                let vision = format!("reference_vision_{i}");
                let apply = format!("reference_apply_{i}");
                graph[&load] = json!({"class_type":"LoadImage","inputs":{"image":filename}});
                graph[&vision] = json!({"class_type":"CLIPVisionEncode","inputs":{
                    "clip_vision":["reference_vision",0],"image":[load,0],"crop":"center"}});
                graph[&apply] = json!({"class_type":"StyleModelApply","inputs":{
                    "conditioning":conditioning,"style_model":["reference_style",0],
                    "clip_vision_output":[vision,0],"strength":1.0,"strength_type":"multiply"}});
                conditioning = json!([apply, 0]);
            }
            graph["5"]["inputs"]["positive"] = conditioning;
        }
        _ => bail!("this workflow does not produce an image"),
    }
    Ok(graph)
}

pub fn video_workflow(
    model: &str,
    companions: &WanCompanions,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
    settings: &ResolvedGenSettings,
    images: &[String],
) -> Result<Value> {
    check_count(images)?;
    let mut graph = comfyui::video_workflow(model, companions, prompt, params, seed, settings)?;
    let width = graph["6"]["inputs"]["width"].clone();
    let height = graph["6"]["inputs"]["height"].clone();
    let length = graph["6"]["inputs"]["length"].clone();
    let mut positive = json!(["4", 0]);
    let mut negative = json!(["5", 0]);
    let mut last = String::new();
    for (i, filename) in images.iter().enumerate() {
        let load = format!("reference_load_{i}");
        let vace = format!("reference_vace_{i}");
        graph[&load] = json!({"class_type":"LoadImage","inputs":{"image":filename}});
        // WanVaceToVideo takes only the first image of a batch. Chaining its
        // conditioning appends independent VACE contexts consumed by Wan.
        graph[&vace] = json!({"class_type":"WanVaceToVideo","inputs":{
            "positive":positive,"negative":negative,"vae":["3",0],
            "width":width,"height":height,"length":length,"batch_size":1,
            "strength":1.0 / images.len() as f64,"reference_image":[load,0]}});
        positive = json!([vace, 0]);
        negative = json!([vace, 1]);
        last = vace;
    }
    graph["8"]["inputs"]["positive"] = positive;
    graph["8"]["inputs"]["negative"] = negative;
    graph["8"]["inputs"]["latent_image"] = json!([last, 2]);
    graph["reference_trim"] = json!({"class_type":"TrimVideoLatent","inputs":{
        "samples":["8",0],"trim_amount":[last,3]}});
    graph["9"]["inputs"]["samples"] = json!(["reference_trim", 0]);
    Ok(graph)
}

fn check_count(images: &[String]) -> Result<()> {
    if images.is_empty() || images.len() > MULTI_REFERENCE_LIMIT as usize {
        bail!("this Kronn reference workflow takes 1 to {MULTI_REFERENCE_LIMIT} images");
    }
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn redux_consumes_each_source_in_order_in_the_sampler_conditioning() {
        for count in 1..=3 {
            let images: Vec<_> = (0..count).map(|i| format!("source-{i}.png")).collect();
            let graph = image_workflow(
                ComfyFamily::FluxSchnell,
                "flux",
                "p",
                &MediaParams::default(),
                7,
                &ComfyFamily::FluxSchnell.defaults(),
                ImageReferences {
                    images: &images,
                    denoise: 0.5,
                },
            )
            .unwrap();
            assert_eq!(
                graph["5"]["inputs"]["positive"],
                json!([format!("reference_apply_{}", count - 1), 0])
            );
            for (i, filename) in images.iter().enumerate() {
                assert_eq!(
                    graph[format!("reference_load_{i}")]["inputs"]["image"],
                    *filename
                );
                assert_eq!(
                    graph[format!("reference_apply_{i}")]["inputs"]["clip_vision_output"],
                    json!([format!("reference_vision_{i}"), 0])
                );
                let previous = if i == 0 {
                    "2".into()
                } else {
                    format!("reference_apply_{}", i - 1)
                };
                assert_eq!(
                    graph[format!("reference_apply_{i}")]["inputs"]["conditioning"],
                    json!([previous, 0])
                );
            }
        }
    }

    #[test]
    fn vace_keeps_three_independent_contexts_and_trims_the_reference_latent() {
        let images = vec!["a.png".into(), "b.png".into(), "c.png".into()];
        let graph = video_workflow(
            "vace",
            &WanCompanions {
                text_encoder: "t5".into(),
                vae: "vae".into(),
            },
            "p",
            &MediaParams::default(),
            3,
            &ComfyFamily::Wan21Vace.defaults(),
            &images,
        )
        .unwrap();
        assert_eq!(
            graph["8"]["inputs"]["positive"],
            json!(["reference_vace_2", 0])
        );
        for i in 0..3 {
            assert_eq!(
                graph[format!("reference_load_{i}")]["inputs"]["image"],
                images[i]
            );
            assert_eq!(
                graph[format!("reference_vace_{i}")]["inputs"]["reference_image"],
                json!([format!("reference_load_{i}"), 0])
            );
        }
        assert_eq!(
            graph["reference_vace_2"]["inputs"]["positive"],
            json!(["reference_vace_1", 0])
        );
        assert_eq!(
            graph["9"]["inputs"]["samples"],
            json!(["reference_trim", 0])
        );
        assert_eq!(
            graph["reference_trim"]["inputs"]["trim_amount"],
            json!(["reference_vace_2", 3])
        );
    }

    #[test]
    fn unsupported_counts_and_a_denoise_that_discards_the_source_are_refused() {
        let settings = ComfyFamily::Sdxl.defaults();
        for (images, denoise) in [(vec!["a".into(), "b".into()], 0.5), (vec!["a".into()], 1.0)] {
            assert!(image_workflow(
                ComfyFamily::Sdxl,
                "sdxl",
                "p",
                &MediaParams::default(),
                0,
                &settings,
                ImageReferences {
                    images: &images,
                    denoise
                }
            )
            .is_err());
        }
        assert!(check_count(&[]).is_err());
        assert!(check_count(&vec!["x".into(); 4]).is_err());
        let graph = image_workflow(
            ComfyFamily::Sdxl,
            "sdxl",
            "p",
            &MediaParams::default(),
            0,
            &settings,
            ImageReferences {
                images: &["photo.png".into()],
                denoise: 0.5,
            },
        )
        .unwrap();
        assert_eq!(
            graph["5"]["inputs"]["latent_image"],
            json!(["reference_encode", 0])
        );
        assert_eq!(graph["5"]["inputs"]["denoise"], 0.5);
    }
}
