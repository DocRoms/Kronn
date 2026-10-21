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
    Wan21Vace,
}

impl ComfyFamily {
    pub fn modality(self) -> MediaModality {
        match self {
            Self::FluxSchnell | Self::Sdxl => MediaModality::Image,
            Self::Wan21T2v | Self::Wan21Vace => MediaModality::Video,
        }
    }
}

/// Files whose workflow was measured on 2026-10-19 (KT-1381). Exact names:
/// a checkpoint is matched by what it is, not by what its name suggests.
pub const KNOWN_MODELS: &[(&str, &str, ComfyFamily)] = &[
    (
        VIDEO_FOLDER,
        "wan2.1_vace_1.3B_fp16.safetensors",
        ComfyFamily::Wan21Vace,
    ),
    (
        VIDEO_FOLDER,
        "wan2.1_vace_14B_fp16.safetensors",
        ComfyFamily::Wan21Vace,
    ),
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

/// One file of a suggested download, with the exact size the source serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestedFile {
    pub folder: &'static str,
    pub file: &'static str,
    /// Pinned to a revision, so the bytes cannot change under the hash.
    pub url: &'static str,
    pub bytes: u64,
    /// SHA-256 of the file, as Hugging Face's LFS pointer declares it.
    pub sha256: &'static str,
}

/// A model the ComfyUI card offers to download, like the Ollama suggestions:
/// each one measured on 2026-10-19 (docs/research/comfyui-local-media-2026-10-19.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Suggestion {
    pub id: &'static str,
    pub family: ComfyFamily,
    pub files: &'static [SuggestedFile],
}

impl Suggestion {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|file| file.bytes).sum()
    }
}

pub const SUGGESTIONS: &[Suggestion] = &[
    Suggestion {
        id: "realvisxl-v5",
        family: ComfyFamily::Sdxl,
        files: &[SuggestedFile {
            folder: IMAGE_FOLDER,
            file: "RealVisXL_V5.0_fp16.safetensors",
            url: "https://huggingface.co/SG161222/RealVisXL_V5.0/resolve/ac93e0dda1f6d448cae19bbfab8c5e720a5e48bc/RealVisXL_V5.0_fp16.safetensors",
            bytes: 6_938_065_488,
            sha256: "6a35a7855770ae9820a3c931d4964c3817b6d9e3c6f9c4dabb5b3a94e5643b80",
        }],
    },
    Suggestion {
        id: "flux1-schnell-fp8",
        family: ComfyFamily::FluxSchnell,
        files: &[SuggestedFile {
            folder: IMAGE_FOLDER,
            file: "flux1-schnell-fp8.safetensors",
            url: "https://huggingface.co/Comfy-Org/flux1-schnell/resolve/c2b683ea00713d6feadcd54b39e3725bbc78638b/flux1-schnell-fp8.safetensors",
            bytes: 17_236_328_572,
            sha256: "ead426278b49030e9da5df862994f25ce94ab2ee4df38b556ddddb3db093bf72",
        }],
    },
    Suggestion {
        id: "wan21-t2v-1.3b",
        family: ComfyFamily::Wan21T2v,
        files: &[
            SuggestedFile {
                folder: VIDEO_FOLDER,
                file: "wan2.1_t2v_1.3B_fp16.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.1_ComfyUI_repackaged/resolve/123acf1cc74bccbb9bfff8ac1ee72edc08c2341d/split_files/diffusion_models/wan2.1_t2v_1.3B_fp16.safetensors",
                bytes: 2_838_303_560,
                sha256: "be531024cd9018cb5b48c40cfbb6a6191645b1c792eb8bf4f8c1c6e10f924dc5",
            },
            SuggestedFile {
                folder: TEXT_ENCODER_FOLDER,
                file: "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.1_ComfyUI_repackaged/resolve/123acf1cc74bccbb9bfff8ac1ee72edc08c2341d/split_files/text_encoders/umt5_xxl_fp8_e4m3fn_scaled.safetensors",
                bytes: 6_735_906_897,
                sha256: "c3355d30191f1f066b26d93fba017ae9809dce6c627dda5f6a66eaa651204f68",
            },
            SuggestedFile {
                folder: VAE_FOLDER,
                file: "wan_2.1_vae.safetensors",
                url: "https://huggingface.co/Comfy-Org/Wan_2.1_ComfyUI_repackaged/resolve/123acf1cc74bccbb9bfff8ac1ee72edc08c2341d/split_files/vae/wan_2.1_vae.safetensors",
                bytes: 253_815_318,
                sha256: "2fc39d31359a4b0a64f55876d8ff7fa8d780956ae2cb13463b0223e15148976b",
            },
        ],
    },
];

/// RealVisXL's own recommended negative prompt, plus the terms that kept the
/// render photographic in the 2026-10-19 runs.
const SDXL_NEGATIVE: &str =
    "bad hands, bad anatomy, ugly, deformed, (face asymmetry, eyes asymmetry, \
     deformed eyes, deformed mouth, open mouth), cartoon, illustration, 3d render, cgi, plastic, \
     text, watermark";
const WAN_NEGATIVE: &str = "blurry, low quality, distorted, static, worst quality";

/// The folder a family's main file sits in.
pub fn folder_of(family: ComfyFamily) -> &'static str {
    match family.modality() {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    }
}

/// The family a model belongs to, in this order: the workflow the user chose
/// for this file in the card, the architecture its header declares, the
/// measured file list. `None` = Kronn does not know how to run it yet.
pub fn family_of(
    folder: &str,
    file: &str,
    architecture: Option<&str>,
    chosen: Option<ComfyFamily>,
) -> Option<ComfyFamily> {
    if let Some(family) = chosen.filter(|family| folder_of(*family) == folder) {
        return Some(family);
    }
    if let Some(arch) = architecture.map(str::trim) {
        if folder == IMAGE_FOLDER {
            if arch.eq_ignore_ascii_case("Flux.1-schnell") {
                return Some(ComfyFamily::FluxSchnell);
            }
            // modelspec's SDXL base value; a refiner only refines.
            let arch = arch.to_ascii_lowercase();
            if arch.starts_with("stable-diffusion-xl") && !arch.contains("refiner") {
                return Some(ComfyFamily::Sdxl);
            }
        }
    }
    KNOWN_MODELS
        .iter()
        .find(|(known_folder, known_file, _)| *known_folder == folder && *known_file == file)
        .map(|(_, _, family)| *family)
}

/// Generation settings a user may override per slot in the card. Empty =
/// the family's measured default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ComfyGenSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub denoise: Option<f32>,
}

impl ComfyGenSettings {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The settings a workflow actually runs with.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ResolvedGenSettings {
    pub steps: u32,
    pub cfg: f32,
    pub sampler: String,
    pub scheduler: String,
}

impl ComfyFamily {
    /// The settings measured on 2026-10-19 for this family.
    pub fn defaults(self) -> ResolvedGenSettings {
        let (steps, cfg, sampler, scheduler) = match self {
            Self::FluxSchnell => (4, 1.0, "euler", "simple"),
            Self::Sdxl => (30, 5.0, "dpmpp_sde", "karras"),
            Self::Wan21T2v | Self::Wan21Vace => (30, 6.0, "uni_pc", "simple"),
        };
        ResolvedGenSettings {
            steps,
            cfg,
            sampler: sampler.into(),
            scheduler: scheduler.into(),
        }
    }

    /// The family defaults with the user's overrides on top.
    pub fn settings(self, overrides: &ComfyGenSettings) -> ResolvedGenSettings {
        let defaults = self.defaults();
        let text = |value: &Option<String>, fallback: String| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or(fallback)
        };
        ResolvedGenSettings {
            steps: overrides
                .steps
                .filter(|steps| (1..=200).contains(steps))
                .unwrap_or(defaults.steps),
            cfg: overrides
                .cfg
                .filter(|cfg| cfg.is_finite() && (0.0..=30.0).contains(cfg))
                .unwrap_or(defaults.cfg),
            sampler: text(&overrides.sampler, defaults.sampler),
            scheduler: text(&overrides.scheduler, defaults.scheduler),
        }
    }
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

/// The text-to-image workflow (ComfyUI API format) for a checkpoint of a
/// known family, with the settings to run it with.
pub fn image_workflow(
    family: ComfyFamily,
    checkpoint: &str,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
    settings: &ResolvedGenSettings,
) -> Result<Value> {
    let (width, height) = size_for(IMAGE_SIZES, params.aspect_ratio.as_deref())?;
    let (latent, negative) = match family {
        ComfyFamily::FluxSchnell => ("EmptySD3LatentImage", ""),
        ComfyFamily::Sdxl => ("EmptyLatentImage", SDXL_NEGATIVE),
        ComfyFamily::Wan21T2v | ComfyFamily::Wan21Vace => {
            bail!("{checkpoint} is a video model, not an image one")
        }
    };
    Ok(json!({
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": checkpoint}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": prompt}},
        "3": {"class_type": "CLIPTextEncode", "inputs": {"clip": ["1", 1], "text": negative}},
        "4": {"class_type": latent, "inputs": {"width": width, "height": height, "batch_size": 1}},
        "5": {"class_type": "KSampler", "inputs": {
            "model": ["1", 0], "positive": ["2", 0], "negative": ["3", 0], "latent_image": ["4", 0],
            "seed": seed, "steps": settings.steps, "cfg": settings.cfg,
            "sampler_name": settings.sampler, "scheduler": settings.scheduler, "denoise": 1.0}},
        "6": {"class_type": "VAEDecode", "inputs": {"samples": ["5", 0], "vae": ["1", 2]}},
        "7": {"class_type": "SaveImage", "inputs": {"images": ["6", 0], "filename_prefix": "kronn/image"}}
    }))
}

/// The text-to-video workflow for a Wan 2.1 diffusion model.
pub fn video_workflow(
    diffusion_model: &str,
    companions: &WanCompanions,
    prompt: &str,
    params: &MediaParams,
    seed: u64,
    settings: &ResolvedGenSettings,
) -> Result<Value> {
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
            "seed": seed, "steps": settings.steps, "cfg": settings.cfg,
            "sampler_name": settings.sampler, "scheduler": settings.scheduler, "denoise": 1.0}},
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
        // Settled now: polling again would only wait for the deadline.
        return Ok(HistoryState::Failed(
            "ComfyUI finished the job but saved no output file: the workflow \
             has no save node, or it saved to a temporary folder"
                .into(),
        ));
    }
    Ok(HistoryState::Completed(files))
}

/// Whether ComfyUI's `/queue` (`queue_running` and `queue_pending`, items
/// `[number, prompt_id, …]`) still holds a prompt, and whether it is the one
/// running.
pub fn queue_position(prompt_id: &str, body: &str) -> Result<QueuePosition> {
    let value: Value = serde_json::from_str(body)?;
    let holds = |field: &str| {
        value[field]
            .as_array()
            .into_iter()
            .flatten()
            .any(|item| item[1].as_str() == Some(prompt_id))
    };
    Ok(if holds("queue_running") {
        QueuePosition::Running
    } else if holds("queue_pending") {
        QueuePosition::Pending
    } else {
        QueuePosition::Absent
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueuePosition {
    Running,
    Pending,
    Absent,
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
    fn a_family_comes_from_the_user_the_header_or_the_measured_list_never_the_name() {
        assert_eq!(
            family_of(
                IMAGE_FOLDER,
                "anything.safetensors",
                Some("Flux.1-schnell"),
                None
            ),
            Some(ComfyFamily::FluxSchnell)
        );
        assert_eq!(
            family_of(
                IMAGE_FOLDER,
                "juggernaut.safetensors",
                Some("stable-diffusion-xl-v1-base"),
                None
            ),
            Some(ComfyFamily::Sdxl)
        );
        assert_eq!(
            family_of(
                IMAGE_FOLDER,
                "refiner.safetensors",
                Some("stable-diffusion-xl-v1-refiner"),
                None
            ),
            None
        );
        assert_eq!(
            family_of(IMAGE_FOLDER, "RealVisXL_V5.0_fp16.safetensors", None, None),
            Some(ComfyFamily::Sdxl)
        );
        // A name that merely looks like a known family is not one…
        assert_eq!(
            family_of(IMAGE_FOLDER, "my-flux-finetune.safetensors", None, None),
            None
        );
        // …until the user says which workflow runs it.
        assert_eq!(
            family_of(
                IMAGE_FOLDER,
                "my-flux-finetune.safetensors",
                None,
                Some(ComfyFamily::FluxSchnell)
            ),
            Some(ComfyFamily::FluxSchnell)
        );
        // A choice that does not fit the folder is ignored.
        assert_eq!(
            family_of(
                IMAGE_FOLDER,
                "x.safetensors",
                None,
                Some(ComfyFamily::Wan21T2v)
            ),
            None
        );
        assert_eq!(
            family_of(IMAGE_FOLDER, "wan2.1_t2v_1.3B_fp16.safetensors", None, None),
            None
        );
        assert_eq!(ComfyFamily::Wan21T2v.modality(), MediaModality::Video);
    }

    #[test]
    fn settings_override_the_measured_defaults_only_with_sane_values() {
        let defaults = ComfyFamily::Sdxl.settings(&ComfyGenSettings::default());
        assert_eq!(defaults, ComfyFamily::Sdxl.defaults());
        let tuned = ComfyFamily::Sdxl.settings(&ComfyGenSettings {
            steps: Some(20),
            cfg: Some(4.5),
            sampler: Some(" euler ".into()),
            scheduler: Some("".into()),
            ..Default::default()
        });
        assert_eq!((tuned.steps, tuned.cfg), (20, 4.5));
        assert_eq!(tuned.sampler, "euler");
        assert_eq!(tuned.scheduler, "karras");
        let absurd = ComfyFamily::FluxSchnell.settings(&ComfyGenSettings {
            steps: Some(0),
            cfg: Some(f32::NAN),
            ..Default::default()
        });
        assert_eq!((absurd.steps, absurd.cfg), (4, 1.0));
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
        let flux_settings = ComfyFamily::FluxSchnell.defaults();
        let flux = image_workflow(
            ComfyFamily::FluxSchnell,
            "flux1-schnell-fp8.safetensors",
            "a fox",
            &params(Some("16:9"), None),
            7,
            &flux_settings,
        )
        .unwrap();
        assert_eq!(flux["4"]["class_type"], "EmptySD3LatentImage");
        assert_eq!(flux["4"]["inputs"]["width"], 1344);
        assert_eq!(flux["4"]["inputs"]["height"], 768);
        assert_eq!(flux["5"]["inputs"]["steps"], 4);
        assert_eq!(flux["5"]["inputs"]["seed"], 7);
        assert_eq!(flux["3"]["inputs"]["text"], "");

        let sdxl = image_workflow(
            ComfyFamily::Sdxl,
            "RealVisXL_V5.0_fp16.safetensors",
            "a fox",
            &params(None, None),
            7,
            &ComfyFamily::Sdxl.defaults(),
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

        let video_as_image = image_workflow(
            ComfyFamily::Wan21T2v,
            "wan.safetensors",
            "a fox",
            &params(None, None),
            7,
            &ComfyFamily::Wan21T2v.defaults(),
        )
        .unwrap_err()
        .to_string();
        assert!(video_as_image.contains("video model"), "{video_as_image}");
        // The user's settings reach the sampler.
        let tuned = image_workflow(
            ComfyFamily::Sdxl,
            "juggernaut.safetensors",
            "a fox",
            &params(None, None),
            7,
            &ComfyFamily::Sdxl.settings(&ComfyGenSettings {
                steps: Some(25),
                sampler: Some("euler_ancestral".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        assert_eq!(tuned["5"]["inputs"]["steps"], 25);
        assert_eq!(tuned["5"]["inputs"]["sampler_name"], "euler_ancestral");
        assert_eq!(tuned["5"]["inputs"]["scheduler"], "karras");
        let ratio = image_workflow(
            ComfyFamily::FluxSchnell,
            "flux1-schnell-fp8.safetensors",
            "x",
            &params(Some("5:1"), None),
            7,
            &flux_settings,
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
            &ComfyFamily::Wan21T2v.defaults(),
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
    }

    /// Every suggestion is a model Kronn can run once downloaded: its main
    /// file belongs to the family it claims, in the right folder.
    #[test]
    fn every_suggestion_downloads_a_model_kronn_can_run() {
        for suggestion in SUGGESTIONS {
            let main = suggestion.files[0];
            assert_eq!(
                family_of(main.folder, main.file, None, None),
                Some(suggestion.family),
                "{}",
                suggestion.id
            );
            assert!(suggestion
                .files
                .iter()
                .all(|file| file.url.starts_with("https://huggingface.co/")));
            assert!(suggestion.bytes() > 0);
        }
        let wan = SUGGESTIONS
            .iter()
            .find(|s| s.family == ComfyFamily::Wan21T2v)
            .unwrap();
        let folders: Vec<&str> = wan.files.iter().map(|file| file.folder).collect();
        assert_eq!(folders, [VIDEO_FOLDER, TEXT_ENCODER_FOLDER, VAE_FOLDER]);
        assert!(
            WAN_TEXT_ENCODERS.contains(&wan.files[1].file) && WAN_VAES.contains(&wan.files[2].file)
        );
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

    #[test]
    fn a_finished_job_without_output_is_a_failure_and_the_queue_is_read() {
        let finished = r#"{"p": {"outputs": {}, "status": {"status_str": "success", "completed": true, "messages": []}}}"#;
        match parse_history("p", finished).unwrap() {
            HistoryState::Failed(message) => {
                assert!(message.contains("no output file"), "{message}")
            }
            other => panic!("{other:?}"),
        }
        let queue =
            r#"{"queue_running": [[1, "a", {}, {}, []]], "queue_pending": [[2, "b", {}, {}, []]]}"#;
        assert_eq!(queue_position("a", queue).unwrap(), QueuePosition::Running);
        assert_eq!(queue_position("b", queue).unwrap(), QueuePosition::Pending);
        assert_eq!(queue_position("c", queue).unwrap(), QueuePosition::Absent);
    }

    /// Review — whatever a hostile `/history` names, the file is asked of
    /// the configured ComfyUI's `/view`, its names only as encoded query
    /// values: never another host, never a path.
    #[test]
    fn a_hostile_history_only_ever_reaches_view_on_the_configured_origin() {
        let base = "http://127.0.0.1:8188";
        let history = r#"{"p": {"status": {"status_str": "success", "completed": true, "messages": []},
            "outputs": {"9": {"images": [
                {"filename": "http://evil.example/x.png", "subfolder": "../../..", "type": "output"},
                {"filename": "secret.png", "subfolder": "", "type": "temp"}
            ]}}}}"#;
        let HistoryState::Completed(files) = parse_history("p", history).unwrap() else {
            panic!("completed");
        };
        // A temporary file is never offered.
        assert_eq!(files.len(), 1);
        let url = reqwest::Url::parse(&view_url(base, &files[0]).unwrap()).unwrap();
        assert_eq!(url.origin(), reqwest::Url::parse(base).unwrap().origin());
        assert_eq!(url.path(), "/view");
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["filename"], "http://evil.example/x.png");
        assert_eq!(query["subfolder"], "../../..");
        assert_eq!(query["type"], "output");
    }
}
