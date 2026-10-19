//! ComfyUI as a local media target (KT-1381): detection and installed models
//! for the Settings card, the two media slots, and the in-memory connection
//! media generation uses. ComfyUI is never a row of `external_api_connections`,
//! so it never becomes a chat agent or a mention target.

use std::path::PathBuf;
use std::time::Duration;

use axum::{extract::State, Json};
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value;

use crate::core::comfyui::{
    family_of, wan_companions, ComfyFamily, WanCompanions, CONNECTION_ID, DEFAULT_ENDPOINT,
    IMAGE_FOLDER, TEXT_ENCODER_FOLDER, VAE_FOLDER, VIDEO_FOLDER,
};
use crate::models::{
    ApiResponse, ComfyUiConfig, ComfyUiModel, ComfyUiStatus, ExternalApiConnection,
    ExternalApiConnectionPreset, MediaModality, SetComfyUiSlotsRequest,
};
use crate::AppState;

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const LIST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_METADATA_PROBES: usize = 4;
pub const DOWNLOAD_URL: &str = "https://www.comfy.org/download";
/// Display name of the local source wherever media sources are listed.
pub const DISPLAY_NAME: &str = "ComfyUI (local)";
pub const MENTION_ALIAS: &str = "comfyui";

/// Base URL of the configured ComfyUI: the saved endpoint, else the default
/// local address (reached through `host.docker.internal` from Docker).
pub fn base_url(config: &ComfyUiConfig) -> String {
    if let Some(endpoint) = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
    {
        return endpoint.trim_end_matches('/').to_string();
    }
    if crate::core::env::is_docker() {
        "http://host.docker.internal:8188".to_string()
    } else {
        DEFAULT_ENDPOINT.to_string()
    }
}

fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// ComfyUI's version when its server answers, else `None`.
pub async fn probe_version(base: &str) -> Option<String> {
    let body: Value = client(PROBE_TIMEOUT)
        .get(format!("{base}/system_stats"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    Some(
        body["system"]["comfyui_version"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
    )
}

/// Files ComfyUI holds in one model folder (`GET /models/<folder>`).
pub async fn list_folder(base: &str, folder: &str) -> anyhow::Result<Vec<String>> {
    let files: Vec<String> = client(LIST_TIMEOUT)
        .get(format!("{base}/models/{folder}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(files)
}

/// The architecture a checkpoint's header declares, when it declares one.
async fn declared_architecture(base: &str, folder: &str, file: &str) -> Option<String> {
    let url = reqwest::Url::parse_with_params(
        &format!("{base}/view_metadata/{folder}"),
        [("filename", file)],
    )
    .ok()?;
    let body: Value = client(LIST_TIMEOUT)
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    body["modelspec.architecture"].as_str().map(str::to_string)
}

/// Image and video models installed in ComfyUI, each with its family when
/// Kronn has a measured workflow for it.
pub async fn installed_models(base: &str) -> anyhow::Result<Vec<ComfyUiModel>> {
    let mut models = Vec::new();
    for folder in [IMAGE_FOLDER, VIDEO_FOLDER] {
        let files = list_folder(base, folder).await?;
        let classified = futures::stream::iter(files)
            .map(|file| async move {
                let mut family = family_of(folder, &file, None);
                if family.is_none() && folder == IMAGE_FOLDER {
                    let architecture = declared_architecture(base, folder, &file).await;
                    family = family_of(folder, &file, architecture.as_deref());
                }
                ComfyUiModel {
                    folder: folder.to_string(),
                    modality: family.map(ComfyFamily::modality),
                    family,
                    file,
                }
            })
            .buffered(MAX_METADATA_PROBES)
            .collect::<Vec<_>>()
            .await;
        models.extend(classified);
    }
    Ok(models)
}

/// The text encoder and VAE a Wan video job needs, read from ComfyUI.
pub async fn video_companions(base: &str) -> anyhow::Result<WanCompanions> {
    let text_encoders = list_folder(base, TEXT_ENCODER_FOLDER).await?;
    let vaes = list_folder(base, VAE_FOLDER).await?;
    wan_companions(&text_encoders, &vaes)
}

fn home_dir() -> Option<PathBuf> {
    crate::core::child_env::var("KRONN_HOST_HOME")
        .or_else(|_| crate::core::child_env::var("HOME"))
        .or_else(|_| crate::core::child_env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Whether ComfyUI is installed on this machine while its server is down:
/// Comfy Desktop's app and install folder, or comfy-cli's default workspace.
fn installed_on_disk() -> bool {
    let mut places = vec![PathBuf::from("/Applications/Comfy Desktop.app")];
    if let Some(home) = home_dir() {
        places.push(home.join("ComfyUI-Installs"));
        places.push(home.join("comfy").join("ComfyUI"));
    }
    places.iter().any(|place| place.exists())
}

/// The one-command install, only where it was run and verified (macOS,
/// 2026-10-19). Elsewhere the card links to the official download page.
fn install_command() -> Option<String> {
    cfg!(target_os = "macos").then(|| "brew install --cask comfy".to_string())
}

fn slot(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// GET /api/comfyui/status
pub async fn status(State(state): State<AppState>) -> Json<ApiResponse<ComfyUiStatus>> {
    let config = state.config.read().await.comfyui.clone();
    let base = base_url(&config);
    let image_model = slot(&config.image_model);
    let video_model = slot(&config.video_model);
    let Some(version) = probe_version(&base).await else {
        return Json(ApiResponse::ok(ComfyUiStatus {
            state: if installed_on_disk() {
                "offline"
            } else {
                "not_found"
            }
            .into(),
            endpoint: base,
            version: None,
            models: Vec::new(),
            image_model,
            video_model,
            video_blocker: None,
            install_command: install_command(),
            download_url: DOWNLOAD_URL.into(),
        }));
    };
    let models = match installed_models(&base).await {
        Ok(models) => models,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "ComfyUI answers but its model list could not be read: {error}"
            )))
        }
    };
    let video_blocker = match video_model {
        Some(_) => video_companions(&base)
            .await
            .err()
            .map(|error| error.to_string()),
        None => None,
    };
    Json(ApiResponse::ok(ComfyUiStatus {
        state: "online".into(),
        endpoint: base,
        version: Some(version),
        models,
        image_model,
        video_model,
        video_blocker,
        install_command: install_command(),
        download_url: DOWNLOAD_URL.into(),
    }))
}

/// GET /api/comfyui/source — the local media source the media surfaces list
/// next to the HTTP connections, read from the config only (no network), or
/// `null` while no slot is set.
pub async fn source(State(state): State<AppState>) -> Json<ApiResponse<Option<ExternalApiConnection>>> {
    Json(ApiResponse::ok(local_connection(&state.config.read().await.comfyui)))
}

/// Whether `file` may fill the `modality` slot: a model of a measured family
/// of that modality.
fn check_slot(file: &str, modality: MediaModality) -> Result<(), String> {
    let folder = match modality {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    };
    match family_of(folder, file, None) {
        Some(family) if family.modality() == modality => Ok(()),
        _ => Err(format!(
            "{file} is not a {} model Kronn has a measured workflow for",
            modality.as_str()
        )),
    }
}

/// PUT /api/comfyui/slots
pub async fn set_slots(
    State(state): State<AppState>,
    Json(request): Json<SetComfyUiSlotsRequest>,
) -> Json<ApiResponse<ComfyUiConfig>> {
    let image_model = slot(&request.image_model);
    let video_model = slot(&request.video_model);
    let base = base_url(&state.config.read().await.comfyui);
    // A checkpoint whose header declares a measured architecture is valid
    // even under another file name, so ask ComfyUI before refusing it.
    if let Some(file) = image_model.as_deref() {
        if check_slot(file, MediaModality::Image).is_err() {
            let declared = declared_architecture(&base, IMAGE_FOLDER, file).await;
            if family_of(IMAGE_FOLDER, file, declared.as_deref()).map(ComfyFamily::modality)
                != Some(MediaModality::Image)
            {
                return Json(ApiResponse::err(
                    check_slot(file, MediaModality::Image).unwrap_err(),
                ));
            }
        }
    }
    if let Some(file) = video_model.as_deref() {
        if let Err(message) = check_slot(file, MediaModality::Video) {
            return Json(ApiResponse::err(message));
        }
    }
    let mut config = state.config.write().await;
    let mut next = config.clone();
    next.comfyui.image_model = image_model;
    next.comfyui.video_model = video_model;
    if let Err(error) = crate::core::config::save(&next).await {
        return Json(ApiResponse::err(format!(
            "Failed to save — the previous models are still in effect: {error}"
        )));
    }
    *config = next;
    Json(ApiResponse::ok(config.comfyui.clone()))
}

/// The in-memory connection media generation uses for ComfyUI, when at least
/// one slot is set. Never stored, never listed as an agent.
pub fn local_connection(config: &ComfyUiConfig) -> Option<ExternalApiConnection> {
    let image_model = slot(&config.image_model);
    let video_model = slot(&config.video_model);
    if image_model.is_none() && video_model.is_none() {
        return None;
    }
    let epoch = chrono::DateTime::<Utc>::UNIX_EPOCH;
    Some(ExternalApiConnection {
        id: CONNECTION_ID.to_string(),
        display_name: DISPLAY_NAME.to_string(),
        mention_alias: MENTION_ALIAS.to_string(),
        endpoint: Some(base_url(config)),
        credential_slug: String::new(),
        origin_preset: ExternalApiConnectionPreset::Other,
        economy_model: None,
        default_model: None,
        reasoning_model: None,
        created_at: epoch,
        updated_at: epoch,
        image_model,
        video_model,
        media_endpoint: None,
        decision_model: None,
    })
}

/// What the local workflows accept, stated by Kronn rather than read from a
/// provider catalogue: the measured sizes and durations, no source image, no
/// soundtrack.
pub fn capabilities(
    model: &str,
    modality: MediaModality,
) -> crate::agents::media_capabilities::MediaModelCapabilities {
    use crate::core::comfyui::{IMAGE_SIZES, VIDEO_DURATIONS_SECS, VIDEO_SIZES};
    let ratios = |table: &[(&str, u32, u32)]| {
        table
            .iter()
            .map(|(ratio, _, _)| ratio.to_string())
            .collect::<Vec<_>>()
    };
    crate::agents::media_capabilities::MediaModelCapabilities {
        model: model.to_string(),
        modality,
        durations_secs: match modality {
            MediaModality::Image => Vec::new(),
            MediaModality::Video => VIDEO_DURATIONS_SECS.to_vec(),
        },
        resolutions: match modality {
            MediaModality::Image => Vec::new(),
            MediaModality::Video => vec!["480p".to_string()],
        },
        aspect_ratios: match modality {
            MediaModality::Image => ratios(IMAGE_SIZES),
            MediaModality::Video => ratios(VIDEO_SIZES),
        },
        frame_positions: Vec::new(),
        max_input_references: Some(0),
        generate_audio: Some(false),
    }
}

/// Whether a connection is the local ComfyUI one.
pub fn is_local(connection: &ExternalApiConnection) -> bool {
    connection.id == CONNECTION_ID
}

/// Whether a caller's reference (id, alias with or without `@`, or display
/// name) names the local ComfyUI.
pub fn names_local(reference: &str) -> bool {
    let reference = reference.trim();
    reference == CONNECTION_ID
        || reference
            .trim_start_matches('@')
            .eq_ignore_ascii_case(MENTION_ALIAS)
        || reference.eq_ignore_ascii_case(DISPLAY_NAME)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn the_local_connection_exists_only_with_a_slot_and_never_carries_a_credential() {
        assert!(local_connection(&ComfyUiConfig::default()).is_none());
        assert!(local_connection(&ComfyUiConfig {
            image_model: Some("   ".into()),
            ..Default::default()
        })
        .is_none());
        let connection = local_connection(&ComfyUiConfig {
            image_model: Some(" RealVisXL_V5.0_fp16.safetensors ".into()),
            endpoint: Some("http://127.0.0.1:9000/".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(connection.id, CONNECTION_ID);
        assert!(is_local(&connection));
        assert_eq!(
            connection.image_model.as_deref(),
            Some("RealVisXL_V5.0_fp16.safetensors")
        );
        assert_eq!(connection.video_model, None);
        assert_eq!(
            connection.endpoint.as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert!(connection.credential_slug.is_empty());
        // No text tier: it can never be offered as a chat agent.
        assert!(connection.default_model.is_none() && connection.economy_model.is_none());
    }

    #[test]
    fn the_local_source_is_named_by_id_alias_or_display_name() {
        for reference in [
            "local:comfyui",
            "comfyui",
            "@ComfyUI",
            "ComfyUI (local)",
            " comfyui ",
        ] {
            assert!(names_local(reference), "{reference}");
        }
        assert!(!names_local("openrouter"));
        assert!(!names_local("local:ollama"));
    }

    #[test]
    fn a_slot_only_takes_a_measured_model_of_its_modality() {
        assert!(check_slot("RealVisXL_V5.0_fp16.safetensors", MediaModality::Image).is_ok());
        assert!(check_slot("wan2.1_t2v_1.3B_fp16.safetensors", MediaModality::Video).is_ok());
        assert!(check_slot("wan2.1_t2v_1.3B_fp16.safetensors", MediaModality::Image).is_err());
        let unknown = check_slot("Juggernaut-XL_v9.safetensors", MediaModality::Image).unwrap_err();
        assert!(unknown.contains("measured workflow"), "{unknown}");
    }
}
