// ComfyUI as a local media target (KT-1381): its saved settings and what the
// Settings card reads. The workflows and model families live in
// `core::comfyui`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use std::collections::BTreeMap;

use crate::core::comfyui::{ComfyFamily, ComfyGenSettings, ResolvedGenSettings};
use crate::models::MediaModality;

/// Saved under `[comfyui]` in config.toml. Empty slots mean "ComfyUI is not
/// offered as a media source".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
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
    /// The workflow validated when the image slot was chosen, so a job does
    /// not depend on reading the checkpoint header again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_family: Option<ComfyFamily>,
    /// Where ComfyUI is installed, to tell "installed but stopped" from "not
    /// installed"; `None` = `~/ComfyUI-Installs` (Comfy Desktop's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
    /// The workflow the user chose for a model Kronn cannot recognise on its
    /// own, keyed `<folder>/<file>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_families: BTreeMap<String, ComfyFamily>,
    /// Overrides of the family defaults, per slot.
    #[serde(default, skip_serializing_if = "ComfyGenSettings::is_empty")]
    pub image_settings: ComfyGenSettings,
    #[serde(default, skip_serializing_if = "ComfyGenSettings::is_empty")]
    pub video_settings: ComfyGenSettings,
}

impl ComfyUiConfig {
    /// The workflow the user chose for one model file, if any.
    pub fn chosen_family(&self, folder: &str, file: &str) -> Option<ComfyFamily> {
        self.model_families
            .get(&format!("{folder}/{file}"))
            .copied()
    }
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
    /// `None` = Kronn does not know how to run it until the user picks its
    /// workflow.
    pub family: Option<ComfyFamily>,
    pub modality: Option<MediaModality>,
    /// The family comes from the user's choice, not from the file.
    pub chosen: bool,
}

/// What the ComfyUI card shows.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
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
    /// Models the card offers to download (KT-1385).
    pub suggestions: Vec<ComfyUiSuggestion>,
    /// The folder checked for an installation, as configured or defaulted.
    pub install_dir: String,
    /// What the user typed, `None` while the defaults above apply.
    pub configured_endpoint: Option<String>,
    pub configured_install_dir: Option<String>,
    pub image_settings: ComfyGenSettings,
    pub video_settings: ComfyGenSettings,
    /// Each family's measured defaults, shown as the placeholders.
    pub family_defaults: Vec<ComfyUiFamilyDefaults>,
    /// What this ComfyUI's KSampler accepts (empty while it is down).
    pub samplers: Vec<String>,
    pub schedulers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ComfyUiFamilyDefaults {
    pub family: ComfyFamily,
    pub settings: ResolvedGenSettings,
}

/// PUT /api/comfyui/settings — the machine-dependent part of the card.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetComfyUiSettingsRequest {
    /// Empty = the default local address.
    pub endpoint: Option<String>,
    /// Empty = `~/ComfyUI-Installs`.
    pub install_dir: Option<String>,
    #[serde(default)]
    pub image_settings: ComfyGenSettings,
    #[serde(default)]
    pub video_settings: ComfyGenSettings,
}

/// PUT /api/comfyui/model-family — run a model Kronn cannot recognise with
/// one of its workflows; `family: null` forgets the choice.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetComfyUiModelFamilyRequest {
    pub folder: String,
    pub file: String,
    pub family: Option<ComfyFamily>,
}

/// One suggested model, as the card shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
pub struct ComfyUiSuggestion {
    pub id: String,
    pub family: ComfyFamily,
    pub modality: MediaModality,
    /// Total size of its files, as the source serves them.
    #[ts(type = "number")]
    pub bytes: u64,
    /// Every file is already in ComfyUI.
    pub installed: bool,
    /// Kronn can write into ComfyUI's model folders from here (a native
    /// install; not from Docker, where those paths are the host's).
    pub downloadable: bool,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PullComfyUiModelRequest {
    pub suggestion: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct SetComfyUiSlotsRequest {
    pub image_model: Option<String>,
    pub video_model: Option<String>,
}
