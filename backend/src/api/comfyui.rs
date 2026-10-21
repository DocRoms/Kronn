//! ComfyUI as a local media target (KT-1381): detection and installed models
//! for the Settings card, the two media slots, and the in-memory connection
//! media generation uses. ComfyUI is never a row of `external_api_connections`,
//! so it never becomes a chat agent or a mention target.

use std::path::PathBuf;
use std::time::Duration;

use axum::{
    extract::State,
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value;

use crate::core::comfyui::{
    family_of, wan_companions, ComfyFamily, WanCompanions, CONNECTION_ID, DEFAULT_ENDPOINT,
    IMAGE_FOLDER, TEXT_ENCODER_FOLDER, VAE_FOLDER, VIDEO_FOLDER,
};
use crate::models::{
    ApiResponse, ComfyUiConfig, ComfyUiFamilyDefaults, ComfyUiModel, ComfyUiStatus,
    ComfyUiSuggestion, ExternalApiConnection, ExternalApiConnectionPreset, MediaModality,
    PullComfyUiModelRequest, SetComfyUiModelFamilyRequest, SetComfyUiSettingsRequest,
    SetComfyUiSlotsRequest,
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

/// Whether ComfyUI still holds a submitted prompt, in its history or its
/// queue. `false` means it was lost (ComfyUI restarted, or its queue was
/// cleared): no poll will ever see it finish.
pub async fn prompt_known(base: &str, prompt_id: &str) -> anyhow::Result<bool> {
    let in_history = || async {
        let body = client(LIST_TIMEOUT)
            .get(format!("{base}/history/{prompt_id}"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        anyhow::Ok(
            serde_json::from_str::<Value>(&body)?
                .get(prompt_id)
                .is_some(),
        )
    };
    if in_history().await? {
        return Ok(true);
    }
    let queue = client(LIST_TIMEOUT)
        .get(format!("{base}/queue"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    if crate::core::comfyui::queue_position(prompt_id, &queue)?
        != crate::core::comfyui::QueuePosition::Absent
    {
        return Ok(true);
    }
    // It may have finished between the two reads.
    in_history().await
}

/// What happened to a prompt Kronn gave up on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abandoned {
    /// Cancelled by id: removed from the queue or stopped while rendering.
    Cancelled,
    /// Already finished or unknown to ComfyUI.
    Gone,
    /// Rendering on a ComfyUI that cannot stop one job by id: left to
    /// finish, since `/interrupt` without an id stops whatever runs.
    LeftRunning,
}

/// Stops a prompt Kronn gave up on, by id only. `POST /api/jobs/<id>/cancel`
/// (recent ComfyUI) does it atomically; elsewhere a queued prompt is deleted
/// by id and a rendering one is left running rather than interrupted
/// globally, which could stop another job started in between.
pub async fn abandon_prompt(base: &str, prompt_id: &str) -> anyhow::Result<Abandoned> {
    let http = client(LIST_TIMEOUT);
    let cancel = http
        .post(format!("{base}/api/jobs/{prompt_id}/cancel"))
        .send()
        .await?;
    if cancel.status().is_success() {
        let body: Value = cancel.json().await.unwrap_or(Value::Null);
        if let Some(cancelled) = body["cancelled"].as_bool() {
            return Ok(if cancelled {
                Abandoned::Cancelled
            } else {
                Abandoned::Gone
            });
        }
    }
    let queue = http
        .get(format!("{base}/queue"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(
        match crate::core::comfyui::queue_position(prompt_id, &queue)? {
            crate::core::comfyui::QueuePosition::Pending => {
                http.post(format!("{base}/queue"))
                    .json(&serde_json::json!({ "delete": [prompt_id] }))
                    .send()
                    .await?
                    .error_for_status()?;
                Abandoned::Cancelled
            }
            crate::core::comfyui::QueuePosition::Running => Abandoned::LeftRunning,
            crate::core::comfyui::QueuePosition::Absent => Abandoned::Gone,
        },
    )
}

/// After a job was cancelled or timed out: if it ran on the local ComfyUI,
/// stop it there, and say on the job when that was not possible.
pub async fn release_local_job(state: &AppState, job_id: &str) {
    let id = job_id.to_string();
    let job = match state
        .db
        .with_read_conn(move |conn| crate::db::media_jobs::get(conn, &id))
        .await
    {
        Ok(Some(job)) => job,
        _ => return,
    };
    let Some(handle) = job
        .provider_job_id
        .as_deref()
        .filter(|handle| !handle.is_empty() && job.connection_id == CONNECTION_ID)
    else {
        return;
    };
    let base = base_url(&state.config.read().await.comfyui);
    let note = match abandon_prompt(&base, handle).await {
        Ok(Abandoned::Cancelled | Abandoned::Gone) => return,
        Ok(Abandoned::LeftRunning) => {
            "stopped in Kronn; this ComfyUI cannot stop one job by id, so it finishes \
             rendering on the GPU (update ComfyUI to free it at once)"
                .to_string()
        }
        Err(e) => {
            tracing::warn!(job = %job_id, error = %e, "could not stop the abandoned ComfyUI job");
            format!("stopped in Kronn; ComfyUI could not be asked to stop it: {e}")
        }
    };
    let id = job_id.to_string();
    let _ = state
        .db
        .with_conn(move |conn| crate::db::media_jobs::note_error(conn, &id, &note))
        .await;
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

/// The family a model runs with: the workflow the user chose for it, the
/// measured list, else the architecture its header declares (checkpoints
/// only). `None` until the user picks one in the card.
pub async fn model_family(
    base: &str,
    config: &ComfyUiConfig,
    folder: &str,
    file: &str,
) -> Option<ComfyFamily> {
    let chosen = config.chosen_family(folder, file);
    if let Some(family) = family_of(folder, file, None, chosen) {
        return Some(family);
    }
    if folder != IMAGE_FOLDER {
        return None;
    }
    let architecture = declared_architecture(base, folder, file).await;
    family_of(folder, file, architecture.as_deref(), None)
}

/// Image and video models installed in ComfyUI, each with the family it runs
/// with when Kronn knows one.
pub async fn installed_models(
    base: &str,
    config: &ComfyUiConfig,
) -> anyhow::Result<Vec<ComfyUiModel>> {
    let mut models = Vec::new();
    for folder in [IMAGE_FOLDER, VIDEO_FOLDER] {
        let files = list_folder(base, folder).await?;
        let classified = futures::stream::iter(files)
            .map(|file| async move {
                let chosen = config.chosen_family(folder, &file);
                let family = model_family(base, config, folder, &file).await;
                ComfyUiModel {
                    folder: folder.to_string(),
                    modality: family.map(ComfyFamily::modality),
                    chosen: chosen.is_some() && family == chosen,
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

pub(crate) fn reference_family(
    config: &ComfyUiConfig,
    model: &str,
    modality: MediaModality,
) -> Option<ComfyFamily> {
    let folder = match modality {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    };
    config
        .chosen_family(folder, model)
        .or_else(|| {
            config.image_family.filter(|_| {
                modality == MediaModality::Image && config.image_model.as_deref() == Some(model)
            })
        })
        .or_else(|| family_of(folder, model, None, None))
}

pub async fn reference_capabilities(
    config: &ComfyUiConfig,
    model: &str,
    modality: MediaModality,
) -> crate::agents::media_capabilities::MediaModelCapabilities {
    use crate::core::comfyui_references::{MULTI_REFERENCE_LIMIT, REDUX_MODEL, REDUX_VISION};
    use crate::models::{MediaReferenceLimitSource, MediaReferenceSupport};
    let mut result = capabilities(model, modality);
    let folder = match modality {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    };
    let family = reference_family(config, model, modality);
    let (maximum, required, mut blocker): (u32, &[&str], Option<String>) = match family {
        Some(ComfyFamily::Sdxl) => (1, &["LoadImage", "VAEEncode"], None),
        Some(ComfyFamily::FluxSchnell) => (MULTI_REFERENCE_LIMIT, &["LoadImage", "StyleModelLoader", "CLIPVisionLoader", "CLIPVisionEncode", "StyleModelApply"], None),
        Some(ComfyFamily::Wan21Vace) => (MULTI_REFERENCE_LIMIT, &["LoadImage", "WanVaceToVideo", "TrimVideoLatent"], None),
        _ => (0, &[], Some("Kronn's selected ComfyUI workflow does not take source images. Select SDXL img2img, FLUX Redux, or a Wan VACE video model; this is a workflow limitation, not a statement about every local model.".into())),
    };
    let base = base_url(config);
    if maximum > 0 {
        let checks = futures::future::join_all(required.iter().map(|node| {
            let url = format!("{base}/object_info/{node}");
            let node = node.to_string();
            async move {
                let response = client(PROBE_TIMEOUT)
                    .get(url)
                    .send()
                    .await
                    .ok()?
                    .error_for_status()
                    .ok()?
                    .json::<Value>()
                    .await
                    .ok()?;
                response.get(&node).map(|_| ())
            }
        }))
        .await;
        let missing: Vec<_> = required
            .iter()
            .zip(checks)
            .filter_map(|(node, found)| found.is_none().then_some(*node))
            .collect();
        if !missing.is_empty() {
            blocker = Some(format!("ComfyUI is unavailable or required nodes are missing: {}. Start or update ComfyUI, then check again.",missing.join(", ")));
        } else if !list_folder(&base, folder)
            .await
            .unwrap_or_default()
            .iter()
            .any(|file| file == model)
        {
            blocker = Some(format!("The selected model {folder}/{model} is missing or ComfyUI is unavailable. Add the model or select an installed one, then check again."));
        } else if family == Some(ComfyFamily::FluxSchnell) {
            let (styles, visions) = tokio::join!(
                list_folder(&base, "style_models"),
                list_folder(&base, "clip_vision")
            );
            let mut missing = Vec::new();
            if !styles
                .unwrap_or_default()
                .iter()
                .any(|name| name == REDUX_MODEL)
            {
                missing.push(format!("style_models/{REDUX_MODEL}"));
            }
            if !visions
                .unwrap_or_default()
                .iter()
                .any(|name| name == REDUX_VISION)
            {
                missing.push(format!("clip_vision/{REDUX_VISION}"));
            }
            if !missing.is_empty() {
                blocker = Some(format!(
                    "FLUX Redux requires {}. Add these model files to ComfyUI, then check again.",
                    missing.join(", ")
                ));
            }
        } else if family == Some(ComfyFamily::Wan21Vace) {
            if let Err(error) = video_companions(&base).await {
                blocker = Some(format!("Wan VACE dependencies: {error}"));
            }
        }
    }
    result.max_input_references = Some(if blocker.is_none() { maximum } else { 0 });
    result.reference_support = Some(MediaReferenceSupport {
        workflow: crate::core::comfyui_references::workflow_id(family).map(str::to_string),
        source: MediaReferenceLimitSource::Workflow,
        advertised_max: Some(maximum),
        configured_max: None,
        blocker,
    });
    result
}

fn home_dir() -> Option<PathBuf> {
    crate::core::child_env::var("KRONN_HOST_HOME")
        .or_else(|_| crate::core::child_env::var("HOME"))
        .or_else(|_| crate::core::child_env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// `dir` as shown to the user: the home directory as `~`, so a placeholder
/// never prints one person's own path.
fn shown_dir(dir: &std::path::Path) -> String {
    shown_under(dir, home_dir().as_deref())
}

fn shown_under(dir: &std::path::Path, home: Option<&std::path::Path>) -> String {
    match home.and_then(|home| dir.strip_prefix(home).ok().map(PathBuf::from)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

/// Where ComfyUI is looked for when its server does not answer: the folder
/// the user set, else Comfy Desktop's default `~/ComfyUI-Installs`. A leading
/// `~/` is the home directory.
pub fn install_dir(config: &ComfyUiConfig) -> PathBuf {
    let home = home_dir().unwrap_or_default();
    match config
        .install_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
    {
        Some(dir) => match dir.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(dir),
        },
        None => home.join("ComfyUI-Installs"),
    }
}

/// Samplers and schedulers this ComfyUI's KSampler accepts.
async fn sampler_choices(base: &str) -> (Vec<String>, Vec<String>) {
    let body: Option<Value> = async {
        client(LIST_TIMEOUT)
            .get(format!("{base}/object_info/KSampler"))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()
    }
    .await;
    let list = |field: &str| {
        body.as_ref()
            .and_then(|body| body["KSampler"]["input"]["required"][field][0].as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    (list("sampler_name"), list("scheduler"))
}

fn family_defaults() -> Vec<ComfyUiFamilyDefaults> {
    [
        ComfyFamily::FluxSchnell,
        ComfyFamily::Sdxl,
        ComfyFamily::Wan21T2v,
        ComfyFamily::Wan21Vace,
    ]
    .into_iter()
    .map(|family| ComfyUiFamilyDefaults {
        family,
        settings: family.defaults(),
    })
    .collect()
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
    let dir = install_dir(&config);
    let mut status = ComfyUiStatus {
        state: if dir.is_dir() { "offline" } else { "not_found" }.into(),
        endpoint: base.clone(),
        version: None,
        models: Vec::new(),
        image_model: slot(&config.image_model),
        video_model: slot(&config.video_model),
        video_blocker: None,
        install_command: install_command(),
        download_url: DOWNLOAD_URL.into(),
        suggestions: suggestion_views(&[], false),
        install_dir: shown_dir(&dir),
        configured_endpoint: slot(&config.endpoint),
        configured_install_dir: slot(&config.install_dir),
        image_settings: config.image_settings.clone(),
        video_settings: config.video_settings.clone(),
        family_defaults: family_defaults(),
        samplers: Vec::new(),
        schedulers: Vec::new(),
    };
    let Some(version) = probe_version(&base).await else {
        return Json(ApiResponse::ok(status));
    };
    status.state = "online".into();
    status.version = Some(version);
    status.models = match installed_models(&base, &config).await {
        Ok(models) => models,
        Err(error) => {
            return Json(ApiResponse::err(format!(
                "ComfyUI answers but its model list could not be read: {error}"
            )))
        }
    };
    if status.video_model.is_some() {
        status.video_blocker = video_companions(&base)
            .await
            .err()
            .map(|error| error.to_string());
    }
    status.suggestions = suggestion_views(
        &installed_files(&base).await,
        downloadable(&base, &config).await,
    );
    (status.samplers, status.schedulers) = sampler_choices(&base).await;
    Json(ApiResponse::ok(status))
}

/// GET /api/comfyui/source — the local media source the media surfaces list
/// next to the HTTP connections, read from the config only (no network), or
/// `null` while no slot is set.
pub async fn source(
    State(state): State<AppState>,
) -> Json<ApiResponse<Option<ExternalApiConnection>>> {
    Json(ApiResponse::ok(local_connection(
        &state.config.read().await.comfyui,
    )))
}

/// Files ComfyUI holds in every folder a suggestion writes to.
async fn installed_files(base: &str) -> Vec<(String, String)> {
    let mut installed = Vec::new();
    for folder in [IMAGE_FOLDER, VIDEO_FOLDER, TEXT_ENCODER_FOLDER, VAE_FOLDER] {
        for file in list_folder(base, folder).await.unwrap_or_default() {
            installed.push((folder.to_string(), file));
        }
    }
    installed
}

fn suggestion_views(installed: &[(String, String)], downloadable: bool) -> Vec<ComfyUiSuggestion> {
    crate::core::comfyui::SUGGESTIONS
        .iter()
        .map(|suggestion| ComfyUiSuggestion {
            id: suggestion.id.to_string(),
            family: suggestion.family,
            modality: suggestion.family.modality(),
            bytes: suggestion.bytes(),
            installed: !installed.is_empty()
                && suggestion.files.iter().all(|file| {
                    installed
                        .iter()
                        .any(|(folder, name)| folder == file.folder && name == file.file)
                }),
            downloadable,
        })
        .collect()
}

/// The directory ComfyUI reads `folder` from, as it reports it
/// (`/internal/folder_paths`): the first one that exists on this machine.
/// `None` from Docker, where those are the host's paths.
/// Whether the address is this machine: only then are the folders ComfyUI
/// reports folders of this disk.
fn is_loopback(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        None => false,
    }
}

/// The first folder ComfyUI reports for `folder` that is a real directory
/// inside the install folder, as a path relative to it (symlinks resolved
/// for this check only). Anything else, a folder elsewhere on the disk
/// included, is never written to.
fn confined_model_dir(
    paths: &Value,
    folder: &str,
    install_root: &std::path::Path,
) -> Option<(PathBuf, PathBuf)> {
    let root = install_root.canonicalize().ok()?;
    let dir = paths[folder]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|dir| std::path::Path::new(dir).canonicalize().ok())
        .find(|dir| dir.is_dir() && dir.starts_with(&root))?;
    let rel = dir.strip_prefix(&root).ok()?.to_path_buf();
    Some((root, rel))
}

/// Where a model of `folder` is downloaded, if Kronn may write it: a
/// ComfyUI on this machine (not Docker, whose paths are the host's) and a
/// folder inside the configured install folder, as (install folder, path
/// below it).
async fn model_dir(base: &str, config: &ComfyUiConfig, folder: &str) -> Option<(PathBuf, PathBuf)> {
    if cfg!(not(unix)) || crate::core::env::is_docker() || !is_loopback(base) {
        return None;
    }
    let paths: Value = client(LIST_TIMEOUT)
        .get(format!("{base}/internal/folder_paths"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    confined_model_dir(&paths, folder, &install_dir(config))
}

async fn downloadable(base: &str, config: &ComfyUiConfig) -> bool {
    model_dir(base, config, IMAGE_FOLDER).await.is_some()
}

type PullStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<Event, std::convert::Infallible>> + Send>>;

fn pull_event(kind: &str, data: Value) -> Result<Event, std::convert::Infallible> {
    Ok(Event::default().event(kind).data(data.to_string()))
}

fn pull_error(message: impl Into<String>) -> Sse<PullStream> {
    let message = message.into();
    let stream: PullStream = Box::pin(futures::stream::once(async move {
        pull_event("error", serde_json::json!({ "message": message }))
    }));
    Sse::new(crate::core::sse_limits::bounded(stream))
}

/// Room kept free on the disk beyond the download itself.
#[cfg_attr(not(unix), allow(dead_code))]
const DISK_MARGIN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// POST /api/comfyui/pull — downloads a suggested model into ComfyUI's own
/// model folders, streamed as `progress` events then one `success` or
/// `error` (the Ollama pull contract). Each file lands as `<name>.part`,
/// resumes from there, and is renamed only once its size matches what the
/// source serves. Closing the request stops the download; the `.part` stays
/// for the next attempt.
pub async fn pull(
    State(state): State<AppState>,
    Json(request): Json<PullComfyUiModelRequest>,
) -> Sse<PullStream> {
    let Some(suggestion) = crate::core::comfyui::SUGGESTIONS
        .iter()
        .find(|suggestion| suggestion.id == request.suggestion)
    else {
        return pull_error(format!("unknown suggestion {}", request.suggestion));
    };
    let config = state.config.read().await.comfyui.clone();
    let base = base_url(&config);
    if probe_version(&base).await.is_none() {
        return pull_error(format!(
            "ComfyUI does not answer on {base}: start it, then retry."
        ));
    }
    pull_into(suggestion, &base, &config).await
}

#[cfg(not(unix))]
async fn pull_into(
    _suggestion: &crate::core::comfyui::Suggestion,
    _base: &str,
    _config: &ComfyUiConfig,
) -> Sse<PullStream> {
    pull_error("Download the files from Hugging Face on this system.")
}

#[cfg(unix)]
async fn pull_into(
    suggestion: &crate::core::comfyui::Suggestion,
    base: &str,
    config: &ComfyUiConfig,
) -> Sse<PullStream> {
    use crate::core::model_dir::ModelDir;
    use std::sync::Arc;

    let installed = installed_files(base).await;
    let mut plan = Vec::new();
    for file in suggestion.files {
        if installed
            .iter()
            .any(|(folder, name)| folder == file.folder && name == file.file)
        {
            continue;
        }
        let Some((root, rel)) = model_dir(base, config, file.folder).await else {
            return pull_error(
                "Kronn writes models only into a ComfyUI running on this machine, inside its \
                 install folder: download the files from Hugging Face instead.",
            );
        };
        // Opened now, from the install folder down, and held to the end:
        // every operation is relative to this handle, never to the path.
        let handle = match ModelDir::open_under(&root, &rel) {
            Ok(handle) => Arc::new(handle),
            Err(e) => {
                return pull_error(format!(
                    "{}: {e}; nothing was written",
                    root.join(&rel).display()
                ))
            }
        };
        plan.push((*file, handle, root.join(rel)));
    }
    // One download per file at a time: two writers on one `.part` corrupt it.
    let Some(lease) = PullLease::take(
        plan.iter()
            .map(|(file, _, dir)| dir.join(file.file))
            .collect(),
    ) else {
        return pull_error("This model is already downloading.");
    };
    let total: u64 = plan.iter().map(|(file, _, _)| file.bytes).sum();
    let already: u64 = plan
        .iter()
        .filter_map(|(file, handle, _)| handle.part_len(&part_name(file.file)).ok())
        .sum();
    if let Some((_, _, dir)) = plan.first() {
        let free = fs2::available_space(dir).unwrap_or(0);
        if free < total.saturating_sub(already) + DISK_MARGIN_BYTES {
            return pull_error(format!(
                "Not enough disk space: {} GB needed, {} GB free.",
                (total.saturating_sub(already) + DISK_MARGIN_BYTES) / 1_000_000_000,
                free / 1_000_000_000
            ));
        }
    }
    let stream: PullStream = Box::pin(async_stream::stream! {
        let _lease = lease;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        let mut done_before: u64 = 0;
        for (file, handle, _) in plan {
            match download_file(&http, &file, handle.clone(), total, done_before).await {
                Ok(mut progress) => {
                    while let Some(event) = progress.next().await {
                        match event {
                            Ok(completed) => {
                                yield pull_event("progress", serde_json::json!({
                                    "status": format!("downloading {}", file.file),
                                    "completed": completed, "total": total,
                                }));
                            }
                            Err(message) => {
                                yield pull_event("error", serde_json::json!({ "message": message }));
                                return;
                            }
                        }
                    }
                }
                Err(message) => {
                    yield pull_event("error", serde_json::json!({ "message": message }));
                    return;
                }
            }
            let checked = {
                let handle = handle.clone();
                tokio::task::spawn_blocking(move || finish_file(&handle, &file))
                    .await
                    .unwrap_or_else(|e| Err(format!("checking {} failed: {e}", file.file)))
            };
            if let Err(message) = checked {
                yield pull_event("error", serde_json::json!({ "message": message }));
                return;
            }
            done_before += file.bytes;
        }
        yield pull_event("success", serde_json::json!({ "status": "success", "completed": total, "total": total }));
    });
    Sse::new(crate::core::sse_limits::bounded(stream))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn part_name(file: &str) -> String {
    format!("{file}.part")
}

#[cfg_attr(not(unix), allow(dead_code))]
static PULLING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Holds the target files of one download; released when it ends, however.
#[cfg_attr(not(unix), allow(dead_code))]
struct PullLease(Vec<PathBuf>);

#[cfg_attr(not(unix), allow(dead_code))]
impl PullLease {
    fn take(targets: Vec<PathBuf>) -> Option<Self> {
        let mut pulling = PULLING.lock().unwrap_or_else(|e| e.into_inner());
        if targets.iter().any(|target| pulling.contains(target)) {
            return None;
        }
        pulling.extend(targets.iter().cloned());
        Some(Self(targets))
    }
}

impl Drop for PullLease {
    fn drop(&mut self) {
        let mut pulling = PULLING.lock().unwrap_or_else(|e| e.into_inner());
        pulling.retain(|target| !self.0.contains(target));
    }
}

/// Whether a `Content-Range: bytes <start>-<end>/<total>` answer continues
/// the `.part` exactly: it starts at `start`, ends within the file, and the
/// file has the expected size.
#[cfg_attr(not(unix), allow(dead_code))]
fn range_continues(header: Option<&str>, start: u64, expected_total: u64) -> bool {
    let parsed = || -> Option<(u64, u64, u64)> {
        let (range, total) = header?.strip_prefix("bytes ")?.split_once('/')?;
        let (first, last) = range.split_once('-')?;
        Some((first.parse().ok()?, last.parse().ok()?, total.parse().ok()?))
    };
    matches!(parsed(), Some((first, last, total))
        if first == start && last >= first && last < total && total == expected_total)
}

#[cfg(unix)]
type ByteProgress = std::pin::Pin<Box<dyn futures::Stream<Item = Result<u64, String>> + Send>>;

/// Streams one file into its `.part` inside the model folder, resuming from
/// what is already there. Yields the bytes done across the whole pull, at
/// most every ~1 %. Never writes past the file's size.
#[cfg(unix)]
async fn download_file(
    http: &reqwest::Client,
    file: &crate::core::comfyui::SuggestedFile,
    dir: std::sync::Arc<crate::core::model_dir::ModelDir>,
    total: u64,
    done_before: u64,
) -> Result<ByteProgress, String> {
    use tokio::io::AsyncWriteExt;
    let part = part_name(file.file);
    let fail = |e: std::io::Error| format!("cannot write {part}: {e}");
    let mut resume_from = dir.part_len(&part).map_err(fail)?;
    // Longer than the file: not this file, start over.
    if resume_from > file.bytes {
        dir.remove(&part).map_err(fail)?;
        resume_from = 0;
    }
    if resume_from == file.bytes {
        return Ok(Box::pin(futures::stream::once(async move {
            Ok(done_before + resume_from)
        })));
    }
    let mut request = http.get(file.url);
    if resume_from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|e| format!("download of {} failed to start: {e}", file.file))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("download of {} answered HTTP {status}", file.file));
    }
    // A server that ignored the range sends the whole file again; a range
    // that does not continue the `.part` exactly would splice two files.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if resumed
        && !range_continues(
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            resume_from,
            file.bytes,
        )
    {
        let _ = dir.remove(&part);
        return Err(format!(
            "the source of {} resumed at the wrong place; the partial file was removed, retry",
            file.file
        ));
    }
    let out = dir.open_part(&part, resumed).map_err(fail)?;
    let mut out = tokio::fs::File::from_std(out);
    let mut written = if resumed { resume_from } else { 0 };
    let cap = file.bytes;
    let step = (total / 100).max(1);
    let mut body = response.bytes_stream();
    let label = file.file;
    Ok(Box::pin(async_stream::stream! {
        let mut last_reported = 0_u64;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(e) => { yield Err(format!("download of {label} stopped: {e}")); return; }
            };
            if written + chunk.len() as u64 > cap {
                drop(out);
                let _ = dir.remove(&part);
                yield Err(format!("the source sent more than the {cap} bytes of {label}; the partial file was removed"));
                return;
            }
            if let Err(e) = out.write_all(&chunk).await {
                yield Err(format!("cannot write {label}: {e}"));
                return;
            }
            written += chunk.len() as u64;
            if written - last_reported >= step {
                last_reported = written;
                yield Ok(done_before + written);
            }
        }
        if let Err(e) = out.flush().await {
            yield Err(format!("cannot write {label}: {e}"));
            return;
        }
        yield Ok(done_before + written);
    }))
}

#[cfg_attr(not(unix), allow(dead_code))]
fn sha256_of(mut reader: impl std::io::Read) -> std::io::Result<String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Installs a complete `.part` whose hash matches, or says what is wrong: a
/// short file stays to resume, a wrong or replaced one is removed. The file
/// hashed is the file renamed.
#[cfg(unix)]
fn finish_file(
    dir: &crate::core::model_dir::ModelDir,
    file: &crate::core::comfyui::SuggestedFile,
) -> Result<(), String> {
    let part = part_name(file.file);
    let opened = match dir.open_read(&part) {
        Ok(opened) => opened,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{part} is missing; retry the download"))
        }
        Err(_) => {
            let _ = dir.remove(&part);
            return Err(format!(
                "{part} was replaced during the download; it was removed, retry"
            ));
        }
    };
    let got = opened.metadata().map(|meta| meta.len()).unwrap_or(0);
    if got != file.bytes {
        return Err(format!(
            "{} is {got} bytes instead of {}; it stays as .part and resumes on the next try",
            file.file, file.bytes
        ));
    }
    let id = crate::core::model_dir::file_identity(&opened)
        .map_err(|e| format!("cannot check {part}: {e}"))?;
    let digest = sha256_of(&opened).map_err(|e| format!("cannot check {part}: {e}"))?;
    if !digest.eq_ignore_ascii_case(file.sha256) {
        let _ = dir.remove(&part);
        return Err(format!(
            "{} does not match its published checksum; it was removed, retry the download",
            file.file
        ));
    }
    dir.rename(&part, file.file, id)
        .map_err(|e| format!("cannot finish {}: {e}", file.file))
}

/// Why `file` cannot fill the `modality` slot, if it cannot: it must run
/// with a known workflow of that kind.
async fn slot_family(
    base: &str,
    config: &ComfyUiConfig,
    file: &str,
    modality: MediaModality,
) -> Result<ComfyFamily, String> {
    let folder = match modality {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    };
    match model_family(base, config, folder, file).await {
        Some(family) if family.modality() == modality => Ok(family),
        _ => Err(format!(
            "{file} has no {} workflow yet: choose which one runs it in the ComfyUI card",
            modality.as_str()
        )),
    }
}

/// PUT /api/comfyui/slots — only a slot that changes is checked against
/// ComfyUI, and the image workflow found is saved with it.
pub async fn set_slots(
    State(state): State<AppState>,
    Json(request): Json<SetComfyUiSlotsRequest>,
) -> Json<ApiResponse<ComfyUiConfig>> {
    let image_model = slot(&request.image_model);
    let video_model = slot(&request.video_model);
    let current = state.config.read().await.comfyui.clone();
    let base = base_url(&current);
    let mut image_family = current.image_family;
    if image_model != slot(&current.image_model) {
        image_family = None;
        if let Some(file) = image_model.as_deref() {
            match slot_family(&base, &current, file, MediaModality::Image).await {
                Ok(family) => image_family = Some(family),
                Err(message) => return Json(ApiResponse::err(message)),
            }
        }
    }
    if video_model != slot(&current.video_model) {
        if let Some(file) = video_model.as_deref() {
            if let Err(message) = slot_family(&base, &current, file, MediaModality::Video).await {
                return Json(ApiResponse::err(message));
            }
        }
    }
    save_comfyui(&state, |comfyui| {
        comfyui.image_model = image_model;
        comfyui.image_family = image_family;
        comfyui.video_model = video_model;
    })
    .await
}

/// Saves the family found for the image slot's model when none is saved
/// yet. Best effort: the job already has its family.
pub async fn remember_image_family(state: &AppState, model: &str, family: ComfyFamily) {
    {
        let config = state.config.read().await;
        let slot_model = slot(&config.comfyui.image_model);
        if slot_model.as_deref() != Some(model) || config.comfyui.image_family.is_some() {
            return;
        }
    }
    let model = model.to_string();
    let saved = save_comfyui(state, move |comfyui| {
        if slot(&comfyui.image_model).as_deref() == Some(model.as_str())
            && comfyui.image_family.is_none()
        {
            comfyui.image_family = Some(family);
        }
    })
    .await;
    if !saved.0.success {
        tracing::warn!(error = ?saved.0.error, "could not save the ComfyUI image family");
    }
}

/// Applies one change to `[comfyui]` and saves it; the in-memory config
/// moves only once the file is written.
async fn save_comfyui(
    state: &AppState,
    change: impl FnOnce(&mut ComfyUiConfig),
) -> Json<ApiResponse<ComfyUiConfig>> {
    let mut config = state.config.write().await;
    let mut next = config.clone();
    change(&mut next.comfyui);
    if let Err(error) = crate::core::config::save(&next).await {
        return Json(ApiResponse::err(format!(
            "Failed to save — the previous settings are still in effect: {error}"
        )));
    }
    *config = next;
    Json(ApiResponse::ok(config.comfyui.clone()))
}

fn optional_text(value: &Option<String>) -> Option<String> {
    slot(value)
}

/// PUT /api/comfyui/settings — address, install folder and per-slot
/// generation settings, all machine-dependent.
pub async fn set_settings(
    State(state): State<AppState>,
    Json(request): Json<SetComfyUiSettingsRequest>,
) -> Json<ApiResponse<ComfyUiConfig>> {
    let endpoint = optional_text(&request.endpoint);
    if let Some(endpoint) = endpoint.as_deref() {
        match reqwest::Url::parse(endpoint) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some() => {}
            _ => {
                return Json(ApiResponse::err(format!(
                    "{endpoint} is not an http(s) address such as http://127.0.0.1:8188"
                )))
            }
        }
    }
    let install_dir = optional_text(&request.install_dir);
    for (label, settings) in [
        ("image", &request.image_settings),
        ("video", &request.video_settings),
    ] {
        if settings
            .denoise
            .is_some_and(|value| !value.is_finite() || !(0.0..1.0).contains(&value))
        {
            return Json(ApiResponse::err(format!(
                "{label} img2img denoise must be at least 0 and less than 1"
            )));
        }
        if settings
            .steps
            .is_some_and(|steps| !(1..=200).contains(&steps))
        {
            return Json(ApiResponse::err(format!(
                "{label} steps must be between 1 and 200"
            )));
        }
        if settings
            .cfg
            .is_some_and(|cfg| !cfg.is_finite() || !(0.0..=30.0).contains(&cfg))
        {
            return Json(ApiResponse::err(format!(
                "{label} guidance must be between 0 and 30"
            )));
        }
    }
    let image_settings = request.image_settings.clone();
    let video_settings = request.video_settings.clone();
    save_comfyui(&state, move |comfyui| {
        comfyui.endpoint = endpoint;
        comfyui.install_dir = install_dir;
        comfyui.image_settings = image_settings;
        comfyui.video_settings = video_settings;
    })
    .await
}

/// PUT /api/comfyui/model-family — run a model Kronn cannot recognise with
/// one of its workflows, or forget that choice.
pub async fn set_model_family(
    State(state): State<AppState>,
    Json(request): Json<SetComfyUiModelFamilyRequest>,
) -> Json<ApiResponse<ComfyUiConfig>> {
    let folder = request.folder.trim().to_string();
    let file = request.file.trim().to_string();
    if ![IMAGE_FOLDER, VIDEO_FOLDER].contains(&folder.as_str()) || file.is_empty() {
        return Json(ApiResponse::err(format!(
            "a workflow is chosen for a file of {IMAGE_FOLDER} or {VIDEO_FOLDER}"
        )));
    }
    if let Some(family) = request.family {
        if crate::core::comfyui::folder_of(family) != folder {
            return Json(ApiResponse::err(format!(
                "a {} workflow cannot run a file of {folder}",
                family.modality().as_str()
            )));
        }
    }
    let key = format!("{folder}/{file}");
    let family = request.family;
    save_comfyui(&state, move |comfyui| {
        match family {
            Some(family) => {
                comfyui.model_families.insert(key, family);
            }
            None => {
                comfyui.model_families.remove(&key);
            }
        }
        // The image slot runs the workflow chosen here from now on.
        if folder == IMAGE_FOLDER && slot(&comfyui.image_model).as_deref() == Some(file.as_str()) {
            comfyui.image_family = family;
        }
    })
    .await
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
        reference_support: None,
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

    #[tokio::test]
    async fn reference_limits_require_installed_models_and_nodes_and_recover_after_installation() {
        use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let config = ComfyUiConfig {
            endpoint: Some(server.uri()),
            ..Default::default()
        };
        let model = "flux1-schnell-fp8.safetensors";
        let caps = reference_capabilities(&config, model, MediaModality::Image).await;
        assert_eq!(caps.max_input_references, Some(0));
        assert!(caps
            .reference_support
            .unwrap()
            .blocker
            .unwrap()
            .contains("required nodes"));
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
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({node:{}})),
                )
                .mount(&server)
                .await;
        }
        Mock::given(path("/models/checkpoints"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([model])))
            .mount(&server)
            .await;
        let caps = reference_capabilities(&config, model, MediaModality::Image).await;
        assert_eq!(caps.max_input_references, Some(0));
        assert!(caps
            .reference_support
            .unwrap()
            .blocker
            .unwrap()
            .contains("style_models/flux1-redux-dev"));
        for (folder, files) in [
            ("style_models", vec!["flux1-redux-dev.safetensors"]),
            (
                "clip_vision",
                vec!["sigclip_vision_patch14_384.safetensors"],
            ),
            (
                "diffusion_models",
                vec!["wan2.1_vace_1.3B_fp16.safetensors"],
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
        for (model, modality, workflow) in [
            (model, MediaModality::Image, "flux-redux-v1"),
            (
                "wan2.1_vace_1.3B_fp16.safetensors",
                MediaModality::Video,
                "wan21-vace-references-v1",
            ),
        ] {
            let caps = reference_capabilities(&config, model, modality).await;
            assert_eq!(caps.max_input_references, Some(3), "{caps:?}");
            let support = caps.reference_support.unwrap();
            assert_eq!(support.workflow.as_deref(), Some(workflow));
            assert!(support.blocker.is_none());
        }
    }

    #[test]
    fn the_install_folder_is_shown_under_a_tilde_never_with_the_home_path() {
        let home = std::path::Path::new("/Users/someone");
        assert_eq!(
            shown_under(&home.join("ComfyUI-Installs"), Some(home)),
            "~/ComfyUI-Installs"
        );
        assert_eq!(shown_under(home, Some(home)), "~");
        assert_eq!(
            shown_under(std::path::Path::new("/opt/comfy"), Some(home)),
            "/opt/comfy"
        );
        assert_eq!(
            shown_under(std::path::Path::new("/opt/comfy"), None),
            "/opt/comfy"
        );
    }

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

    #[cfg(unix)]
    fn suggested(url: String, bytes: u64, sha256: &str) -> crate::core::comfyui::SuggestedFile {
        crate::core::comfyui::SuggestedFile {
            folder: IMAGE_FOLDER,
            file: "model.safetensors",
            url: Box::leak(url.into_boxed_str()),
            bytes,
            sha256: Box::leak(sha256.to_string().into_boxed_str()),
        }
    }

    #[cfg(unix)]
    fn hex_sha256(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[cfg(unix)]
    async fn drain(mut progress: ByteProgress) -> Result<u64, String> {
        let mut last = 0;
        while let Some(step) = progress.next().await {
            last = step?;
        }
        Ok(last)
    }

    /// A model folder in a scratch dir, opened like a pull opens it.
    #[cfg(unix)]
    fn model_folder() -> (
        tempfile::TempDir,
        PathBuf,
        std::sync::Arc<crate::core::model_dir::ModelDir>,
    ) {
        let scratch = tempfile::tempdir().unwrap();
        let dir = scratch.path().canonicalize().unwrap();
        let handle =
            crate::core::model_dir::ModelDir::open_under(&dir, std::path::Path::new("")).unwrap();
        (scratch, dir, std::sync::Arc::new(handle))
    }

    #[test]
    fn a_content_range_must_continue_the_part_exactly() {
        assert!(range_continues(Some("bytes 100-999/1000"), 100, 1000));
        assert!(range_continues(Some("bytes 100-499/1000"), 100, 1000));
        for refused in [
            "bytes 100-garbage/1000",
            "bytes 100-99/1000",
            "bytes 100-1001/1000",
            "bytes 100-1000/1000",
            "bytes 0-999/1000",
            "bytes 100-999/2000",
            "bytes 100-999/*",
            "100-999/1000",
        ] {
            assert!(!range_continues(Some(refused), 100, 1000), "{refused}");
        }
        assert!(!range_continues(None, 100, 1000));
    }

    /// KT-1385 — a download lands as `.part`, resumes from it with a Range
    /// request, and only a file of the exact size and checksum becomes the
    /// model.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_resumes_from_its_part_and_only_a_verified_file_becomes_the_model() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let body: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let sha = hex_sha256(&body);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .and(header("range", "bytes=400-"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 400-999/1000")
                    .set_body_bytes(body[400..].to_vec()),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let (_scratch, dir, handle) = model_folder();
        let part = dir.join("model.safetensors.part");
        let target = dir.join("model.safetensors");
        let http = reqwest::Client::new();
        let file = suggested(format!("{}/model", server.uri()), 1000, &sha);

        // Resume: 400 bytes already there.
        std::fs::write(&part, &body[..400]).unwrap();
        let done = drain(
            download_file(&http, &file, handle.clone(), 1000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(done, 1000);
        finish_file(&handle, &file).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        assert!(std::fs::symlink_metadata(&target).unwrap().is_file());
        assert!(!part.exists());

        // A short file never becomes the model, and stays for the next try.
        std::fs::remove_file(&target).unwrap();
        let short = suggested(format!("{}/model", server.uri()), 2000, &sha);
        let cut = drain(
            download_file(&http, &short, handle.clone(), 2000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(cut, 1000);
        let refused = finish_file(&handle, &short).unwrap_err();
        assert!(refused.contains("1000 bytes instead of 2000"), "{refused}");
        assert!(!target.exists());
        assert!(part.exists());

        // The right size with the wrong bytes is removed, never installed.
        std::fs::write(&part, vec![0_u8; 1000]).unwrap();
        let corrupt = finish_file(&handle, &file).unwrap_err();
        assert!(corrupt.contains("checksum"), "{corrupt}");
        assert!(!part.exists() && !target.exists());
    }

    /// A source that sends more than the file, a `.part` longer than the
    /// file, or a range that does not continue the `.part`: nothing is
    /// spliced or kept.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_download_never_writes_past_the_file_nor_splices_a_wrong_range() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .and(header("range", "bytes=100-"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 100-1001/1000")
                    .set_body_bytes(vec![1_u8; 902]),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1_u8; 1500]))
            .mount(&server)
            .await;
        let (_scratch, dir, handle) = model_folder();
        let part = dir.join("model.safetensors.part");
        let http = reqwest::Client::new();
        let file = suggested(format!("{}/model", server.uri()), 1000, "");

        // 1500 bytes for a 1000-byte file: stopped and removed.
        let over = drain(
            download_file(&http, &file, handle.clone(), 1000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap_err();
        assert!(over.contains("more than the 1000 bytes"), "{over}");
        assert!(!part.exists());

        // A range that ends past the file, for a resume at 100.
        std::fs::write(&part, vec![1_u8; 100]).unwrap();
        let spliced = download_file(&http, &file, handle.clone(), 1000, 0)
            .await
            .err()
            .unwrap();
        assert!(spliced.contains("wrong place"), "{spliced}");
        assert!(!part.exists());

        // A `.part` longer than the file is not this file: started over
        // (and then refused for the oversized body above).
        std::fs::write(&part, vec![1_u8; 4000]).unwrap();
        let restarted = drain(
            download_file(&http, &file, handle.clone(), 1000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap_err();
        assert!(restarted.contains("more than"), "{restarted}");
    }

    /// Review (KT-1381 r1) — a `.part` planted as a symbolic or hard link to
    /// a file outside, or the folder swapped for a link to elsewhere: nothing
    /// outside is ever written, and the model is never a link.
    #[cfg(unix)]
    #[tokio::test]
    async fn links_planted_in_the_model_folder_never_lead_a_download_outside() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let body = vec![7_u8; 1000];
        let sha = hex_sha256(&body);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("victim");
        let http = reqwest::Client::new();
        let file = suggested(format!("{}/model", server.uri()), 1000, &sha);

        for hard in [false, true] {
            let (_scratch, dir, handle) = model_folder();
            let part = dir.join("model.safetensors.part");
            let target = dir.join("model.safetensors");
            std::fs::write(&victim, b"twenty-five bytes, intact").unwrap();
            if hard {
                std::fs::hard_link(&victim, &part).unwrap();
            } else {
                std::os::unix::fs::symlink(&victim, &part).unwrap();
            }
            drain(
                download_file(&http, &file, handle.clone(), 1000, 0)
                    .await
                    .unwrap(),
            )
            .await
            .unwrap();
            finish_file(&handle, &file).unwrap();
            assert_eq!(
                std::fs::read(&victim).unwrap(),
                b"twenty-five bytes, intact",
                "hard: {hard}"
            );
            let installed = std::fs::symlink_metadata(&target).unwrap();
            assert!(installed.is_file() && std::os::unix::fs::MetadataExt::nlink(&installed) == 1);
            assert_eq!(std::fs::read(&target).unwrap(), body);
        }

        // The `.part` swapped for a link between the download and the finish.
        let (_scratch, dir, handle) = model_folder();
        let part = dir.join("model.safetensors.part");
        std::fs::write(&victim, b"twenty-five bytes, intact").unwrap();
        std::os::unix::fs::symlink(&victim, &part).unwrap();
        let refused = finish_file(&handle, &file).unwrap_err();
        assert!(refused.contains("replaced"), "{refused}");
        assert!(!dir.join("model.safetensors").exists());
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"twenty-five bytes, intact"
        );

        // A folder anywhere below the install folder swapped for a link to
        // elsewhere — the last one or one in between — and `..`: refused,
        // nothing written.
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("models/checkpoints")).unwrap();
        std::fs::create_dir_all(outside.path().join("checkpoints")).unwrap();
        let open = |rel: &str| {
            crate::core::model_dir::ModelDir::open_under(&root, std::path::Path::new(rel))
        };
        assert!(open("models/checkpoints").is_ok());
        std::fs::rename(root.join("models"), root.join("moved")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("models")).unwrap();
        assert!(open("models/checkpoints").is_err());
        std::fs::rename(root.join("moved/checkpoints"), root.join("moved/held")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("checkpoints"),
            root.join("moved/checkpoints"),
        )
        .unwrap();
        assert!(open("moved/checkpoints").is_err());
        assert!(open("moved/../moved/held").is_err());
        assert!(open("moved/held").is_ok());

        // Swapped after the download opened it: writes stay in the folder
        // the handle holds, never in the link's target.
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        let dir = root.join("checkpoints");
        std::fs::create_dir(&dir).unwrap();
        let handle = std::sync::Arc::new(
            crate::core::model_dir::ModelDir::open_under(
                &root,
                std::path::Path::new("checkpoints"),
            )
            .unwrap(),
        );
        let held = scratch.path().join("held");
        std::fs::rename(&dir, &held).unwrap();
        std::os::unix::fs::symlink(outside.path(), &dir).unwrap();
        drain(
            download_file(&http, &file, handle.clone(), 1000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        finish_file(&handle, &file).unwrap();
        assert!(held.join("model.safetensors").is_file());
        assert!(!outside.path().join("model.safetensors").exists());
        assert!(!outside.path().join("model.safetensors.part").exists());
    }

    #[test]
    fn one_model_file_is_downloaded_by_one_pull_at_a_time() {
        let target = PathBuf::from("/models/checkpoints/one-pull-at-a-time.safetensors");
        let first = PullLease::take(vec![target.clone()]).unwrap();
        assert!(PullLease::take(vec![target.clone()]).is_none());
        drop(first);
        assert!(PullLease::take(vec![target]).is_some());
    }

    #[test]
    fn downloads_go_only_into_the_install_folder_of_a_comfyui_on_this_machine() {
        for local in [
            "http://127.0.0.1:8188",
            "http://localhost:8000",
            "http://[::1]:8188",
        ] {
            assert!(is_loopback(local), "{local}");
        }
        for remote in [
            "http://192.168.1.20:8188",
            "https://comfy.example.com",
            "not a url",
        ] {
            assert!(!is_loopback(remote), "{remote}");
        }

        let install = tempfile::tempdir().unwrap();
        let inside = install.path().join("ComfyUI/models/checkpoints");
        std::fs::create_dir_all(&inside).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let paths = |dirs: &[&std::path::Path]| serde_json::json!({ "checkpoints": dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>() });
        // The first folder outside the install folder is skipped.
        assert_eq!(
            confined_model_dir(
                &paths(&[elsewhere.path(), &inside]),
                "checkpoints",
                install.path()
            ),
            Some((
                install.path().canonicalize().unwrap(),
                PathBuf::from("ComfyUI/models/checkpoints")
            ))
        );
        assert_eq!(
            confined_model_dir(&paths(&[elsewhere.path()]), "checkpoints", install.path()),
            None
        );
        // A link inside the install folder that leads out of it is out.
        #[cfg(unix)]
        {
            let link = install.path().join("escape");
            std::os::unix::fs::symlink(elsewhere.path(), &link).unwrap();
            assert_eq!(
                confined_model_dir(&paths(&[&link]), "checkpoints", install.path()),
                None
            );
        }
    }

    #[test]
    fn suggestions_are_installed_only_when_every_file_is_there() {
        let files = |list: &[(&str, &str)]| {
            list.iter()
                .map(|(f, n)| (f.to_string(), n.to_string()))
                .collect::<Vec<_>>()
        };
        let views = suggestion_views(
            &files(&[
                ("checkpoints", "RealVisXL_V5.0_fp16.safetensors"),
                ("diffusion_models", "wan2.1_t2v_1.3B_fp16.safetensors"),
                ("vae", "wan_2.1_vae.safetensors"),
            ]),
            true,
        );
        let installed = |id: &str| views.iter().find(|view| view.id == id).unwrap().installed;
        assert!(installed("realvisxl-v5"));
        assert!(!installed("flux1-schnell-fp8"));
        // The Wan pack still lacks its text encoder.
        assert!(!installed("wan21-t2v-1.3b"));
        assert!(suggestion_views(&[], false)
            .iter()
            .all(|view| !view.installed && !view.downloadable));
    }

    #[test]
    fn the_install_folder_is_configurable_with_a_desktop_default() {
        let home = home_dir().unwrap_or_default();
        assert_eq!(
            install_dir(&ComfyUiConfig::default()),
            home.join("ComfyUI-Installs")
        );
        let at = |dir: &str| ComfyUiConfig {
            install_dir: Some(dir.into()),
            ..Default::default()
        };
        assert_eq!(install_dir(&at("~/apps/comfy")), home.join("apps/comfy"));
        assert_eq!(install_dir(&at("/opt/comfy")), PathBuf::from("/opt/comfy"));
        assert_eq!(install_dir(&at("   ")), home.join("ComfyUI-Installs"));
    }

    /// Nothing answers on this address, so only the user's choice and the
    /// measured list can name a family.
    #[tokio::test]
    async fn a_slot_takes_a_known_model_or_one_whose_workflow_the_user_chose() {
        let base = "http://127.0.0.1:9";
        let mut config = ComfyUiConfig::default();
        assert_eq!(
            slot_family(
                base,
                &config,
                "RealVisXL_V5.0_fp16.safetensors",
                MediaModality::Image
            )
            .await,
            Ok(ComfyFamily::Sdxl)
        );
        assert_eq!(
            slot_family(
                base,
                &config,
                "wan2.1_t2v_1.3B_fp16.safetensors",
                MediaModality::Video
            )
            .await,
            Ok(ComfyFamily::Wan21T2v)
        );
        assert!(slot_family(
            base,
            &config,
            "wan2.1_t2v_1.3B_fp16.safetensors",
            MediaModality::Image
        )
        .await
        .is_err());
        let unknown = slot_family(
            base,
            &config,
            "Juggernaut-XL_v9.safetensors",
            MediaModality::Image,
        )
        .await
        .unwrap_err();
        assert!(unknown.contains("choose which one runs it"), "{unknown}");
        config.model_families.insert(
            "checkpoints/Juggernaut-XL_v9.safetensors".into(),
            ComfyFamily::Sdxl,
        );
        assert_eq!(
            slot_family(
                base,
                &config,
                "Juggernaut-XL_v9.safetensors",
                MediaModality::Image
            )
            .await,
            Ok(ComfyFamily::Sdxl)
        );
    }
}
