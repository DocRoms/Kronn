// ComfyUI as a local media target (KT-1381): its saved settings and what the
// Settings card reads. The workflows and model families live in
// `core::comfyui`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::comfyui::ComfyFamily;
use crate::models::MediaModality;

/// Saved under `[comfyui]` in config.toml. Empty slots mean "ComfyUI is not
/// offered as a media source".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ComfyUiConfig {
    /// Base URL; `None` = the default local address (`http://127.0.0.1:8188`,
    /// `host.docker.internal` from Docker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Checkpoint file used for images (`models/checkpoints`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
    /// Diffusion model file used for videos (`models/diffusion_models`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_model: Option<String>,
}

pub fn is_unset(config: &ComfyUiConfig) -> bool {
    config == &ComfyUiConfig::default()
}

/// One model file ComfyUI reports, and what Kronn can do with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ComfyUiModel {
    /// ComfyUI folder: `checkpoints` (image) or `diffusion_models` (video).
    pub folder: String,
    pub file: String,
    /// `None` = no measured workflow: listed, never selectable.
    pub family: Option<ComfyFamily>,
    pub modality: Option<MediaModality>,
}

/// What the ComfyUI card shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ComfyUiStatus {
    /// `online` (server answers), `offline` (installed, server down) or
    /// `not_found` (nothing detected on this machine).
    pub state: String,
    pub endpoint: String,
    pub version: Option<String>,
    pub models: Vec<ComfyUiModel>,
    pub image_model: Option<String>,
    pub video_model: Option<String>,
    /// Why the chosen video model cannot run yet (a missing text encoder or
    /// VAE), in a sentence; `None` when it can.
    pub video_blocker: Option<String>,
    /// The one-command install for this OS, only where it was run and
    /// verified; elsewhere the card links to the official download page.
    pub install_command: Option<String>,
    pub download_url: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetComfyUiSlotsRequest {
    pub image_model: Option<String>,
    pub video_model: Option<String>,
}
