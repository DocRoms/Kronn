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

fn home_dir() -> Option<PathBuf> {
    crate::core::child_env::var("KRONN_HOST_HOME")
        .or_else(|_| crate::core::child_env::var("HOME"))
        .or_else(|_| crate::core::child_env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
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
        install_dir: dir.display().to_string(),
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
    status.suggestions = suggestion_views(&installed_files(&base).await, downloadable(&base).await);
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
async fn model_dir(base: &str, folder: &str) -> Option<PathBuf> {
    if crate::core::env::is_docker() {
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
    paths[folder]
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .find(|dir| dir.is_dir())
}

async fn downloadable(base: &str) -> bool {
    model_dir(base, IMAGE_FOLDER).await.is_some()
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
    let base = base_url(&state.config.read().await.comfyui);
    if probe_version(&base).await.is_none() {
        return pull_error(format!(
            "ComfyUI does not answer on {base}: start it, then retry."
        ));
    }
    let installed = installed_files(&base).await;
    let mut plan = Vec::new();
    for file in suggestion.files {
        if installed
            .iter()
            .any(|(folder, name)| folder == file.folder && name == file.file)
        {
            continue;
        }
        let Some(dir) = model_dir(&base, file.folder).await else {
            return pull_error(
                "Kronn cannot write into ComfyUI's model folders from here \
                 (Docker, or ComfyUI reported none): download the files from Hugging Face instead.",
            );
        };
        plan.push((*file, dir));
    }
    let total: u64 = plan.iter().map(|(file, _)| file.bytes).sum();
    let already: u64 = plan
        .iter()
        .map(|(file, dir)| part_len(&dir.join(part_name(file.file))))
        .sum();
    if let Some((_, dir)) = plan.first() {
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
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        let mut done_before: u64 = 0;
        for (file, dir) in plan {
            let part = dir.join(part_name(file.file));
            let target = dir.join(file.file);
            match download_file(&http, &file, &part, &target, total, done_before).await {
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
            if let Err(message) = finish_file(&part, &target, file.bytes) {
                yield pull_event("error", serde_json::json!({ "message": message }));
                return;
            }
            done_before += file.bytes;
        }
        yield pull_event("success", serde_json::json!({ "status": "success", "completed": total, "total": total }));
    });
    Sse::new(crate::core::sse_limits::bounded(stream))
}

fn part_name(file: &str) -> String {
    format!("{file}.part")
}

fn part_len(part: &std::path::Path) -> u64 {
    std::fs::metadata(part).map(|meta| meta.len()).unwrap_or(0)
}

type ByteProgress = std::pin::Pin<Box<dyn futures::Stream<Item = Result<u64, String>> + Send>>;

/// Streams one file into its `.part`, resuming from what is already there.
/// Yields the bytes done across the whole pull, at most every ~1 %.
async fn download_file(
    http: &reqwest::Client,
    file: &crate::core::comfyui::SuggestedFile,
    part: &std::path::Path,
    _target: &std::path::Path,
    total: u64,
    done_before: u64,
) -> Result<ByteProgress, String> {
    use tokio::io::AsyncWriteExt;
    let resume_from = part_len(part).min(file.bytes);
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
    // A server that ignored the range sends the whole file again.
    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .append(resumed)
        .write(true)
        .truncate(!resumed)
        .open(part)
        .await
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut written = if resumed { resume_from } else { 0 };
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

/// Renames a complete `.part`, or says what came short — a wrong size never
/// lands as a model file.
fn finish_file(
    part: &std::path::Path,
    target: &std::path::Path,
    expected: u64,
) -> Result<(), String> {
    let got = part_len(part);
    if got != expected {
        return Err(format!(
            "{} is {got} bytes instead of {expected}; it stays as .part and resumes on the next try",
            target.display()
        ));
    }
    std::fs::rename(part, target).map_err(|e| format!("cannot finish {}: {e}", target.display()))
}

/// Why `file` cannot fill the `modality` slot, if it cannot: it must run
/// with a known workflow of that kind.
async fn slot_refusal(
    base: &str,
    config: &ComfyUiConfig,
    file: &str,
    modality: MediaModality,
) -> Option<String> {
    let folder = match modality {
        MediaModality::Image => IMAGE_FOLDER,
        MediaModality::Video => VIDEO_FOLDER,
    };
    match model_family(base, config, folder, file).await {
        Some(family) if family.modality() == modality => None,
        _ => Some(format!(
            "{file} has no {} workflow yet: choose which one runs it in the ComfyUI card",
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
    let current = state.config.read().await.comfyui.clone();
    let base = base_url(&current);
    for (file, modality) in [
        (image_model.as_deref(), MediaModality::Image),
        (video_model.as_deref(), MediaModality::Video),
    ] {
        if let Some(file) = file {
            if let Some(message) = slot_refusal(&base, &current, file, modality).await {
                return Json(ApiResponse::err(message));
            }
        }
    }
    save_comfyui(&state, |comfyui| {
        comfyui.image_model = image_model;
        comfyui.video_model = video_model;
    })
    .await
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
    save_comfyui(&state, move |comfyui| match family {
        Some(family) => {
            comfyui.model_families.insert(key, family);
        }
        None => {
            comfyui.model_families.remove(&key);
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

    fn suggested(url: String, bytes: u64) -> crate::core::comfyui::SuggestedFile {
        crate::core::comfyui::SuggestedFile {
            folder: IMAGE_FOLDER,
            file: "model.safetensors",
            url: Box::leak(url.into_boxed_str()),
            bytes,
        }
    }

    async fn drain(mut progress: ByteProgress) -> Result<u64, String> {
        let mut last = 0;
        while let Some(step) = progress.next().await {
            last = step?;
        }
        Ok(last)
    }

    /// KT-1385 — a download lands as `.part`, resumes from it with a Range
    /// request, and only a file of the exact size becomes the model.
    #[tokio::test]
    async fn a_download_resumes_from_its_part_and_only_a_full_file_becomes_the_model() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let body: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .and(header("range", "bytes=400-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(body[400..].to_vec()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/model"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;

        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("model.safetensors.part");
        let target = dir.path().join("model.safetensors");
        let http = reqwest::Client::new();
        let file = suggested(format!("{}/model", server.uri()), body.len() as u64);

        // Resume: 400 bytes already there.
        std::fs::write(&part, &body[..400]).unwrap();
        let done = drain(
            download_file(&http, &file, &part, &target, 1000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(done, 1000);
        finish_file(&part, &target, 1000).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        assert!(!part.exists());

        // A short file never becomes the model, and stays for the next try.
        std::fs::remove_file(&target).unwrap();
        let short = suggested(format!("{}/model", server.uri()), 2000);
        drain(
            download_file(&http, &short, &part, &target, 2000, 0)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        let refused = finish_file(&part, &target, 2000).unwrap_err();
        assert!(refused.contains("1000 bytes instead of 2000"), "{refused}");
        assert!(!target.exists());
        assert!(part.exists());
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
        assert!(slot_refusal(
            base,
            &config,
            "RealVisXL_V5.0_fp16.safetensors",
            MediaModality::Image
        )
        .await
        .is_none());
        assert!(slot_refusal(
            base,
            &config,
            "wan2.1_t2v_1.3B_fp16.safetensors",
            MediaModality::Video
        )
        .await
        .is_none());
        assert!(slot_refusal(
            base,
            &config,
            "wan2.1_t2v_1.3B_fp16.safetensors",
            MediaModality::Image
        )
        .await
        .is_some());
        let unknown = slot_refusal(
            base,
            &config,
            "Juggernaut-XL_v9.safetensors",
            MediaModality::Image,
        )
        .await
        .unwrap();
        assert!(unknown.contains("choose which one runs it"), "{unknown}");
        config.model_families.insert(
            "checkpoints/Juggernaut-XL_v9.safetensors".into(),
            ComfyFamily::Sdxl,
        );
        assert!(slot_refusal(
            base,
            &config,
            "Juggernaut-XL_v9.safetensors",
            MediaModality::Image
        )
        .await
        .is_none());
    }
}
