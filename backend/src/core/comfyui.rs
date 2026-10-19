//! ComfyUI as a local media target (KT-1381): the model families Kronn has
//! measured, the workflow it submits for each, and the reading of ComfyUI's
//! job history. Pure functions; the HTTP calls live in the codec and the API.
//!
//! ComfyUI runs a node graph, not "prompt + model". A family is therefore a
//! measured workflow, never a guess: a model is usable only when its file
//! declares a known architecture or is listed in [`KNOWN_MODELS`].

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ts_rs::TS;

use crate::models::{MediaModality, MediaParams};

/// Default address of a local ComfyUI (Comfy Desktop and `comfy launch`).
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:8188";

/// Connection id a media job names to use the local ComfyUI. Never a row of
/// `external_api_connections`, so it can never become a chat agent.
pub const CONNECTION_ID: &str = "local:comfyui";

/// Model folders Kronn reads, in ComfyUI's own names.
pub const IMAGE_FOLDER: &str = "checkpoints";
pub const VIDEO_FOLDER: &str = "diffusion_models";
pub const TEXT_ENCODER_FOLDER: &str = "text_encoders";
pub const VAE_FOLDER: &str = "vae";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ComfyFamily {
    /// FLUX.1 schnell all-in-one checkpoint: 4 steps, no negative prompt.
    FluxSchnell,
    /// SDXL all-in-one checkpoint (RealVisXL): 30 steps with a negative prompt.
    Sdxl,
    /// Wan 2.1 text-to-video: diffusion model + umt5 text encoder + VAE.
    Wan21T2v,
}

impl ComfyFamily {
    pub fn modality(self) -> MediaModality {
        match self {
            Self::FluxSchnell | Self::Sdxl => MediaModality::Image,
            Self::Wan21T2v => MediaModality::Video,
        }
    }
}

/// Files whose workflow was measured on 2026-10-19 (KT-1381). Exact names:
/// a checkpoint is matched by what it is, not by what its name suggests.
pub const KNOWN_MODELS: &[(&str, &str, ComfyFamily)] = &[
    (
        IMAGE_FOLDER,
        "flux1-schnell-fp8.safetensors",
        ComfyFamily::FluxSchnell,
    ),
    (
        IMAGE_FOLDER,
        "RealVisXL_V5.0_fp16.safetensors",
        ComfyFamily::Sdxl,
    ),
    (
        VIDEO_FOLDER,
        "wan2.1_t2v_1.3B_fp16.safetensors",
        ComfyFamily::Wan21T2v,
    ),
    (
        VIDEO_FOLDER,
        "wan2.1_t2v_1.3B_bf16.safetensors",
        ComfyFamily::Wan21T2v,
    ),
];

/// Wan 2.1 companion files, in order of preference.
pub const WAN_TEXT_ENCODERS: &[&str] = &[
    "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
    "umt5_xxl_fp16.safetensors",
];
pub const WAN_VAES: &[&str] = &["wan_2.1_vae.safetensors"];

/// RealVisXL's own recommended negative prompt, plus the terms that kept the
/// render photographic in the 2026-10-19 runs.
const SDXL_NEGATIVE: &str =
    "bad hands, bad anatomy, ugly, deformed, (face asymmetry, eyes asymmetry, \
     deformed eyes, deformed mouth, open mouth), cartoon, illustration, 3d render, cgi, plastic, \
     text, watermark";
const WAN_NEGATIVE: &str = "blurry, low quality, distorted, static, worst quality";

/// The family a model belongs to: the architecture its header declares when
/// ComfyUI reports one, else the measured file list. `None` = not usable.
pub fn family_of(folder: &str, file: &str, architecture: Option<&str>) -> Option<ComfyFamily> {
    if let Some(arch) = architecture {
        // The only declared value measured so far (FLUX.1 schnell's header).
        if folder == IMAGE_FOLDER && arch.trim().eq_ignore_ascii_case("Flux.1-schnell") {
            return Some(ComfyFamily::FluxSchnell);
        }
    }
    KNOWN_MODELS
        .iter()
        .find(|(known_folder, known_file, _)| *known_folder == folder && *known_file == file)
        .map(|(_, _, family)| *family)
}

/// Text encoder and VAE a Wan 2.1 job needs, from what is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WanCompanions {
    pub text_encoder: String,
    pub vae: String,
}

/// Picks the companions among installed files, or says which one is missing.
pub fn wan_companions(text_encoders: &[String], vaes: &[String]) -> Result<WanCompanions> {
    let pick = |installed: &[String], wanted: &[&str]| {
        wanted
            .iter()
            .find(|name| installed.iter().any(|file| file == *name))
            .map(|name| (*name).to_string())
    };
    let Some(text_encoder) = pick(text_encoders, WAN_TEXT_ENCODERS) else {
        bail!(
            "ComfyUI has no Wan text encoder: add {} to models/{TEXT_ENCODER_FOLDER}",
            WAN_TEXT_ENCODERS[0]
        );
    };
    let Some(vae) = pick(vaes, WAN_VAES) else {
        bail!(
            "ComfyUI has no Wan VAE: add {} to models/{VAE_FOLDER}",
            WAN_VAES[0]
        );
    };
    Ok(WanCompanions { text_encoder, vae })
}

/// Aspect ratios offered for local images, with the ~1 MP sizes FLUX and SDXL
/// were measured at (multiples of 64).
pub const IMAGE_SIZES: &[(&str, u32, u32)] = &[
    ("1:1", 1024, 1024),
    ("16:9", 1344, 768),
    ("9:16", 768, 1344),
    ("3:2", 1216, 832),
    ("2:3", 832, 1216),
    ("4:3", 1152, 896),
    ("3:4", 896, 1152),
];

/// Wan 2.1 1.3B sizes (480p class, measured at 832×480).
pub const VIDEO_SIZES: &[(&str, u32, u32)] =
    &[("16:9", 832, 480), ("9:16", 480, 832), ("1:1", 624, 624)];

/// Durations offered for local video. Wan renders 16 fps; 2 s took 594 s here.
pub const VIDEO_DURATIONS_SECS: &[u32] = &[2, 3, 4, 5];
pub const VIDEO_FPS: u32 = 16;

fn size_for(table: &[(&str, u32, u32)], aspect_ratio: Option<&str>) -> Result<(u32, u32)> {
    let wanted = aspect_ratio.map(str::trim).unwrap_or(table[0].0);
    table
        .iter()
        .find(|(ratio, _, _)| *ratio == wanted)
        .map(|(_, w, h)| (*w, *h))
        .ok_or_else(|| {
            let offered: Vec<&str> = table.iter().map(|(ratio, _, _)| *ratio).collect();
            anyhow::anyhow!(
                "aspect ratio {wanted} is not offered for local ComfyUI ({})",
                offered.join(", ")
            )
        })
}

/// Frames for a duration at 16 fps, rounded up to Wan's 4n+1 length.
pub fn video_frames(duration_secs: Option<u32>) -> Result<u32> {
    let secs = duration_secs.unwrap_or(VIDEO_DURATIONS_SECS[0]);
    if !VIDEO_DURATIONS_SECS.contains(&secs) {
        bail!(
            "duration {secs}s is not offered for local ComfyUI video ({:?} s)",
            VIDEO_DURATIONS_SECS
        );
    }
    let frames = secs * VIDEO_FPS;
    Ok(frames.div_ceil(4) * 4 + 1)
}

/// A stable seed per job, so a resumed job renders the same picture.
pub fn seed_for(job_id: &str) -> u64 {
    // FNV-1a: stable across builds, unlike the std hasher.
    job_id
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
        & 0x0000_ffff_ffff_ffff
}

/// The text-to-image workflow (ComfyUI API format) for a measured checkpoint.
pub fn image_workflow(
    checkpoint: &str,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
) -> Result<Value> {
    let Some(family) = family_of(IMAGE_FOLDER, checkpoint, None) else {
        bail!(
            "Kronn has no measured workflow for the image model {checkpoint}: \
             pick a known model in the ComfyUI card"
        );
    };
    let (width, height) = size_for(IMAGE_SIZES, params.aspect_ratio.as_deref())?;
    let (latent, negative, steps, cfg, sampler, scheduler) = match family {
        ComfyFamily::FluxSchnell => ("EmptySD3LatentImage", "", 4, 1.0, "euler", "simple"),
        ComfyFamily::Sdxl => (
            "EmptyLatentImage",
            SDXL_NEGATIVE,
            30,
            5.0,
            "dpmpp_sde",
            "karras",
        ),
        ComfyFamily::Wan21T2v => bail!("{checkpoint} is a video model, not an image one"),
    };
    Ok(json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": checkpoint}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": prompt}},
        "3": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": negative}},
        "4": {"class_type": latent, "inputs": {"width": width, "height": height, "batch_size": 1}},
        "5": {"class_type": "KSampler", "inputs": {
            "model": ["1", 0], "positive": ["2", 0], "negative": ["3", 0], "latent_image": ["4", 0],
            "seed": seed, "steps": steps, "cfg": cfg, "sampler_name": sampler,
            "scheduler": scheduler, "denoise": 1.0}},
        "6": {"class_type": "VAEDecode", "inputs": {"samples": ["5", 0], "vae": ["1", 2]}},
        "7": {"class_type": "SaveImage", "inputs": {"images": ["6", 0], "filename_prefix": "kronn/image"}}
    }))
}

/// The text-to-video workflow for a measured Wan 2.1 diffusion model.
pub fn video_workflow(
    diffusion_model: &str,
    companions: &WanCompanions,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
) -> Result<Value> {
    if family_of(VIDEO_FOLDER, diffusion_model, None) != Some(ComfyFamily::Wan21T2v) {
        bail!(
            "Kronn has no measured workflow for the video model {diffusion_model}: \
             pick a known model in the ComfyUI card"
        );
    }
    let (width, height) = size_for(VIDEO_SIZES, params.aspect_ratio.as_deref())?;
    let length = video_frames(params.duration_secs)?;
    Ok(json!({
        "1": {"class_type": "UNETLoader", "inputs": {"unet_name": diffusion_model, "weight_dtype": "default"}},
        "2": {"class_type": "CLIPLoader", "inputs": {"clip_name": companions.text_encoder, "type": "wan"}},
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": companions.vae}},
        "4": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": prompt}},
        "5": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["2", 0], "text": WAN_NEGATIVE}},
        "6": {"class_type": "EmptyHunyuanLatentVideo", "inputs": {"width": width, "height": height, "length": length, "batch_size": 1}},
        "7": {"class_type": "ModelSamplingSD3", "inputs": {"model": ["1", 0], "shift": 8.0}},
        "8": {"class_type": "KSampler", "inputs": {
            "model": ["7", 0], "positive": ["4", 0], "negative": ["5", 0], "latent_image": ["6", 0],
            "seed": seed, "steps": 30, "cfg": 6.0, "sampler_name": "uni_pc",
            "scheduler": "simple", "denoise": 1.0}},
        "9": {"class_type": "VAEDecode", "inputs": {"samples": ["8", 0], "vae": ["3", 0]}},
        "10": {"class_type": "CreateVideo", "inputs": {"images": ["9", 0], "fps": VIDEO_FPS}},
        "11": {"class_type": "SaveVideo", "inputs": {"video": ["10", 0], "filename_prefix": "kronn/video",
            "format": "auto", "format.codec": "auto"}}
    }))
}

/// One produced file, as `/history` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

/// What `/history/<prompt_id>` says about a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryState {
    /// Not in the history yet (queued or running).
    Pending,
    Completed(Vec<OutputFile>),
    Failed(String),
}

/// Reads `/history/<prompt_id>`: `{}` while the job runs, then one entry with
/// `status` and `outputs`.
pub fn parse_history(prompt_id: &str, body: &str) -> Result<HistoryState> {
    let value: Value = serde_json::from_str(body)?;
    let Some(entry) = value.get(prompt_id) else {
        return Ok(HistoryState::Pending);
    };
    let status = &entry["status"];
    if status["status_str"].as_str() == Some("error") {
        let message = status["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|message| message[0].as_str() == Some("execution_error"))
            .and_then(|message| message[1]["exception_message"].as_str())
            .unwrap_or("ComfyUI reported an execution error");
        return Ok(HistoryState::Failed(
            message.trim().chars().take(300).collect(),
        ));
    }
    if status["completed"].as_bool() != Some(true) {
        return Ok(HistoryState::Pending);
    }
    let mut files = Vec::new();
    for output in entry["outputs"]
        .as_object()
        .into_iter()
        .flat_map(|o| o.values())
    {
        // A video is reported under `images` by SaveVideo too; `animated`
        // only flags it.
        for file in output["images"].as_array().into_iter().flatten() {
            let (Some(filename), Some(kind)) = (file["filename"].as_str(), file["type"].as_str())
            else {
                continue;
            };
            if kind != "output" {
                continue;
            }
            files.push(OutputFile {
                filename: filename.to_string(),
                subfolder: file["subfolder"].as_str().unwrap_or_default().to_string(),
                kind: file["metadata"]["kind"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    if files.is_empty() {
        bail!("ComfyUI finished the job but reported no output file");
    }
    Ok(HistoryState::Completed(files))
}

/// Where a produced file is fetched, on the configured ComfyUI itself.
pub fn view_url(base: &str, file: &OutputFile) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        &format!("{}/view", base.trim_end_matches('/')),
        [
            ("filename", file.filename.as_str()),
            ("subfolder", file.subfolder.as_str()),
            ("type", "output"),
        ],
    )?;
    Ok(url.into())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn params(aspect: Option<&str>, duration: Option<u32>) -> MediaParams {
        MediaParams {
            aspect_ratio: aspect.map(str::to_string),
            duration_secs: duration,
            ..Default::default()
        }
    }

    #[test]
    fn a_family_comes_from_the_header_or_the_measured_list_never_the_name() {
        assert_eq!(
            family_of(IMAGE_FOLDER, "anything.safetensors", Some("Flux.1-schnell")),
            Some(ComfyFamily::FluxSchnell)
        );
        assert_eq!(
            family_of(IMAGE_FOLDER, "RealVisXL_V5.0_fp16.safetensors", None),
            Some(ComfyFamily::Sdxl)
        );
        // A name that merely looks like a known family is not one.
        assert_eq!(
            family_of(IMAGE_FOLDER, "my-flux-finetune.safetensors", None),
            None
        );
        assert_eq!(
            family_of(IMAGE_FOLDER, "Juggernaut-XL_v9.safetensors", None),
            None
        );
        // The right file in the wrong folder is not that model.
        assert_eq!(
            family_of(IMAGE_FOLDER, "wan2.1_t2v_1.3B_fp16.safetensors", None),
            None
        );
        assert_eq!(ComfyFamily::Wan21T2v.modality(), MediaModality::Video);
    }

    #[test]
    fn wan_companions_are_picked_from_what_is_installed_or_named_when_missing() {
        let files = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let picked = wan_companions(
            &files(&[
                "umt5_xxl_fp16.safetensors",
                "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
            ]),
            &files(&["wan_2.1_vae.safetensors"]),
        )
        .unwrap();
        assert_eq!(
            picked.text_encoder,
            "umt5_xxl_fp8_e4m3fn_scaled.safetensors"
        );
        assert_eq!(picked.vae, "wan_2.1_vae.safetensors");
        let missing = wan_companions(&files(&[]), &files(&["wan_2.1_vae.safetensors"]))
            .unwrap_err()
            .to_string();
        assert!(
            missing.contains("umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            "{missing}"
        );
        let no_vae = wan_companions(&files(&["umt5_xxl_fp16.safetensors"]), &files(&[]))
            .unwrap_err()
            .to_string();
        assert!(no_vae.contains("wan_2.1_vae.safetensors"), "{no_vae}");
    }

    /// The workflows are the ones measured on 2026-10-19.
    #[test]
    fn image_workflows_follow_the_measured_settings_of_each_family() {
        let flux = image_workflow(
            "flux1-schnell-fp8.safetensors",
            "a fox",
            &params(Some("16:9"), None),
            7,
        )
        .unwrap();
        assert_eq!(flux["4"]["class_type"], "EmptySD3LatentImage");
        assert_eq!(flux["4"]["inputs"]["width"], 1344);
        assert_eq!(flux["4"]["inputs"]["height"], 768);
        assert_eq!(flux["5"]["inputs"]["steps"], 4);
        assert_eq!(flux["5"]["inputs"]["seed"], 7);
        assert_eq!(flux["3"]["inputs"]["text"], "");

        let sdxl = image_workflow(
            "RealVisXL_V5.0_fp16.safetensors",
            "a fox",
            &params(None, None),
            7,
        )
        .unwrap();
        assert_eq!(sdxl["4"]["class_type"], "EmptyLatentImage");
        assert_eq!(sdxl["4"]["inputs"]["width"], 1024);
        assert_eq!(sdxl["5"]["inputs"]["sampler_name"], "dpmpp_sde");
        assert_eq!(sdxl["5"]["inputs"]["scheduler"], "karras");
        assert_eq!(sdxl["5"]["inputs"]["steps"], 30);
        assert!(sdxl["3"]["inputs"]["text"]
            .as_str()
            .unwrap()
            .contains("bad hands"));

        let unknown = image_workflow(
            "Juggernaut-XL_v9.safetensors",
            "a fox",
            &params(None, None),
            7,
        )
        .unwrap_err()
        .to_string();
        assert!(unknown.contains("no measured workflow"), "{unknown}");
        let ratio = image_workflow(
            "flux1-schnell-fp8.safetensors",
            "x",
            &params(Some("5:1"), None),
            7,
        )
        .unwrap_err()
        .to_string();
        assert!(ratio.contains("5:1") && ratio.contains("16:9"), "{ratio}");
    }

    #[test]
    fn video_workflow_uses_wan_lengths_and_the_companions() {
        let companions = WanCompanions {
            text_encoder: "umt5_xxl_fp8_e4m3fn_scaled.safetensors".into(),
            vae: "wan_2.1_vae.safetensors".into(),
        };
        let video = video_workflow(
            "wan2.1_t2v_1.3B_fp16.safetensors",
            &companions,
            "a fox trotting",
            &params(None, Some(2)),
            9,
        )
        .unwrap();
        assert_eq!(video["6"]["inputs"]["length"], 33);
        assert_eq!(video["6"]["inputs"]["width"], 832);
        assert_eq!(video["2"]["inputs"]["type"], "wan");
        assert_eq!(video["3"]["inputs"]["vae_name"], "wan_2.1_vae.safetensors");
        assert_eq!(video["8"]["inputs"]["steps"], 30);
        assert_eq!(video["10"]["inputs"]["fps"], 16);
        assert_eq!(video_frames(Some(4)).unwrap(), 65);
        assert!(video_frames(Some(9)).is_err());
        assert!(video_workflow(
            "flux1-schnell-fp8.safetensors",
            &companions,
            "x",
            &params(None, None),
            9
        )
        .is_err());
    }

    #[test]
    fn a_seed_is_stable_per_job() {
        assert_eq!(seed_for("media-1"), seed_for("media-1"));
        assert_ne!(seed_for("media-1"), seed_for("media-2"));
    }

    // Shapes captured from the 2026-10-19 runs on ComfyUI 0.42.1.
    const RUNNING: &str = r#"{}"#;
    const DONE_IMAGE: &str = r#"{"p1":{"prompt":[],"outputs":{"7":{"images":[{"filename":"kronn-measure_00002_.png","subfolder":"","type":"output","metadata":{"kind":"image","width":1024,"height":1024}}]}},"status":{"status_str":"success","completed":true,"messages":[]}}}"#;
    const DONE_VIDEO: &str = r#"{"p2":{"outputs":{"10":{"images":[{"filename":"kronn-measure-video_00001_.webp","subfolder":"","type":"output"}],"animated":[true]},"12":{"images":[{"filename":"kronn-measure_00001_.mp4","subfolder":"video","type":"output","metadata":{"kind":"video","width":832,"height":480,"duration":2.0625,"fps":16.0,"frame_count":33}}],"animated":[true]}},"status":{"status_str":"success","completed":true,"messages":[]}}}"#;
    const FAILED: &str = r#"{"p3":{"outputs":{},"status":{"status_str":"error","completed":false,"messages":[["execution_start",{}],["execution_error",{"exception_message":"Value not in list: ckpt_name 'gone.safetensors'","node_id":"1"}]]}}}"#;

    #[test]
    fn history_is_read_as_pending_completed_or_failed() {
        assert_eq!(parse_history("p1", RUNNING).unwrap(), HistoryState::Pending);
        let HistoryState::Completed(files) = parse_history("p1", DONE_IMAGE).unwrap() else {
            panic!("a finished image job");
        };
        assert_eq!(files[0].filename, "kronn-measure_00002_.png");
        assert_eq!(files[0].kind, "image");
        let HistoryState::Completed(files) = parse_history("p2", DONE_VIDEO).unwrap() else {
            panic!("a finished video job");
        };
        let video = files.iter().find(|file| file.kind == "video").unwrap();
        assert_eq!(video.subfolder, "video");
        let HistoryState::Failed(message) = parse_history("p3", FAILED).unwrap() else {
            panic!("a failed job");
        };
        assert!(message.contains("gone.safetensors"), "{message}");
    }

    #[test]
    fn view_urls_stay_on_the_configured_comfyui_and_are_encoded() {
        let file = OutputFile {
            filename: "kronn image_00001_.png".into(),
            subfolder: "video".into(),
            kind: "image".into(),
        };
        assert_eq!(
            view_url("http://127.0.0.1:8188/", &file).unwrap(),
            "http://127.0.0.1:8188/view?filename=kronn+image_00001_.png&subfolder=video&type=output"
        );
    }
}
