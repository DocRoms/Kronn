//! KT-943 — the context window a local model can actually be given on this
//! machine, computed from the model rather than from a slice of installed RAM.
//!
//! A flat band ("64 GiB → 65 536 tokens") ignores the two things that decide it:
//! what the weights take, and what one token of context costs the KV cache.
//! The second differs by an order of magnitude between models: `qwen3.8:27b-mlx`
//! caches only 16 of its 64 layers (the rest are linear attention with a
//! constant state), i.e. 64 KiB per token, where a dense 70B caches 320 KiB.
//!
//! The ceiling here is `(GPU budget − weights − margin) / KV bytes per token`,
//! and it is only produced when every input is known. Anything missing — the
//! attention shape, the weights, the machine's memory — returns the coarse band
//! the caller already had, never an invented figure. Everything in this file is
//! pure (the readers of the machine's state sit at the bottom) so each rule is
//! testable without a machine.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// What macOS lets the GPU wire on Apple Silicon when `iogpu.wired_limit_mb` is
/// left at 0: about three quarters of installed memory.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
const APPLE_GPU_SHARE_PERCENT: u64 = 75;
/// Room kept on top of weights and cache for prefill buffers and activations,
/// which grow with the prompt and are not part of either. A tenth of the budget,
/// never below 2 GiB, so a small machine is not left with a rounding error.
const MARGIN_SHARE_PERCENT: u64 = 10;
const MARGIN_FLOOR_BYTES: u64 = 2 * GIB;
/// A manifest lists one layer per tensor (1 209 for `qwen3.8:27b-mlx`, 190 KB);
/// the blobs read here are `config.json` files of a few hundred KB at most.
/// These bounds only keep a corrupt store from being slurped into memory.
const MAX_MANIFEST_BYTES: u64 = 8 * MIB;
const MAX_CONFIG_BYTES: u64 = 16 * MIB;

/// How the server stores the KV cache (`OLLAMA_KV_CACHE_TYPE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KvCacheType {
    F16,
    Q8_0,
    Q4_0,
}

impl KvCacheType {
    /// `None` for a value Kronn cannot price: an estimate built on a guess of
    /// the cache's width would be a made-up figure. Unset or empty is the
    /// server's default, f16.
    pub(crate) fn from_setting(raw: Option<&str>) -> Option<Self> {
        match raw
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("f16") => Some(Self::F16),
            Some("q8_0") => Some(Self::Q8_0),
            Some("q4_0") => Some(Self::Q4_0),
            Some(_) => None,
        }
    }

    /// Bytes `values` cached values take. ggml packs quantised values in blocks
    /// of 32 that carry a 2-byte scale: q8_0 is 34 bytes a block, q4_0 is 18.
    fn bytes_for(self, values: u64) -> u64 {
        match self {
            Self::F16 => values.saturating_mul(2),
            Self::Q8_0 => values.saturating_mul(34).div_ceil(32),
            Self::Q4_0 => values.saturating_mul(18).div_ceil(32),
        }
    }
}

/// What one token of context adds to the cache, as a count of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KvShape {
    /// Keys and values stored for one token, summed over every layer whose cache
    /// grows with the context. Layers with a constant state (linear attention)
    /// or a bounded window add nothing per token and are not counted.
    values_per_token: u64,
}

impl KvShape {
    fn new(cached_heads: u64, key_dim: u64, value_dim: u64) -> Option<Self> {
        let values = cached_heads.checked_mul(key_dim.checked_add(value_dim)?)?;
        (values > 0).then_some(Self {
            values_per_token: values,
        })
    }

    pub(crate) fn bytes_per_token(self, cache: KvCacheType) -> u64 {
        cache.bytes_for(self.values_per_token)
    }
}

fn unsigned(object: &Value, key: &str) -> Option<u64> {
    object.get(key)?.as_u64()
}

/// Layers whose cache grows with the context. `layer_types` says it layer by
/// layer, `full_attention_interval` says it as a period, and without either every
/// layer is counted. A layer kind this does not know, or a windowed model whose
/// pattern is not written down, is not priced: `None`.
fn cached_layers(text: &Value, layers: u64) -> Option<u64> {
    if let Some(kinds) = text.get("layer_types").and_then(Value::as_array) {
        if kinds.len() as u64 != layers {
            return None;
        }
        let mut full = 0;
        for kind in kinds {
            match kind.as_str()? {
                "full_attention" => full += 1,
                "linear_attention" => {}
                _ => return None,
            }
        }
        return Some(full);
    }
    if let Some(interval) = unsigned(text, "full_attention_interval").filter(|n| *n > 0) {
        return Some(layers / interval);
    }
    let windowed = text.get("sliding_window").is_some_and(|w| !w.is_null())
        && text.get("use_sliding_window").and_then(Value::as_bool) != Some(false);
    (!windowed).then_some(layers)
}

/// The attention shape from a Hugging Face `config.json`, the only place an MLX
/// (safetensors) model publishes it: `/api/show` carries neither the KV head
/// count nor the head dimension for those. Multimodal checkpoints nest the
/// language model under `text_config`.
pub(crate) fn kv_shape_from_config(config: &Value) -> Option<KvShape> {
    let text = config
        .get("text_config")
        .filter(|v| v.is_object())
        .unwrap_or(config);
    // Latent attention caches one compressed vector per token, not K and V per head.
    if text.get("kv_lora_rank").is_some_and(|v| !v.is_null()) {
        return None;
    }
    let layers = unsigned(text, "num_hidden_layers")?;
    let kv_heads = unsigned(text, "num_key_value_heads")?;
    let head_dim = unsigned(text, "head_dim").or_else(|| {
        let heads = unsigned(text, "num_attention_heads").filter(|h| *h > 0)?;
        Some(unsigned(text, "hidden_size")? / heads)
    })?;
    let cached = cached_layers(text, layers)?;
    KvShape::new(cached.checked_mul(kv_heads)?, head_dim, head_dim)
}

/// The attention shape from an `/api/show` answer, for the GGUF models that
/// publish it as `<arch>.block_count`, `<arch>.attention.head_count_kv` and
/// friends.
pub(crate) fn kv_shape_from_show(show: &Value) -> Option<KvShape> {
    let info = show.get("model_info")?.as_object()?;
    let arch = info.get("general.architecture")?.as_str()?;
    let get = |suffix: &str| info.get(&format!("{arch}.{suffix}"));
    // Windowed or latent attention is not described by per-token arithmetic.
    if get("attention.sliding_window").is_some() || get("attention.kv_lora_rank").is_some() {
        return None;
    }
    let layers = get("block_count")?.as_u64()?;
    let cached_heads = match get("attention.head_count_kv")? {
        // One count per layer, 0 for a layer without attention.
        Value::Array(per_layer) => {
            if per_layer.len() as u64 != layers {
                return None;
            }
            per_layer
                .iter()
                .try_fold(0u64, |sum, heads| sum.checked_add(heads.as_u64()?))?
        }
        heads => {
            let interval = get("full_attention_interval")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0);
            let cached = interval.map_or(layers, |n| layers / n);
            heads.as_u64()?.checked_mul(cached)?
        }
    };
    let heads = get("attention.head_count")
        .and_then(Value::as_u64)
        .filter(|h| *h > 0);
    let dim = |suffix: &str| {
        get(suffix)
            .and_then(Value::as_u64)
            .or_else(|| Some(get("embedding_length")?.as_u64()? / heads?))
    };
    KvShape::new(
        cached_heads,
        dim("attention.key_length")?,
        dim("attention.value_length")?,
    )
}

/// The memory the GPU may use. `iogpu.wired_limit_mb` when it was set (non-zero),
/// else about three quarters of installed memory — never more than the machine has.
#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
pub(crate) fn gpu_budget_bytes(total_ram_bytes: u64, wired_limit_mb: Option<u64>) -> u64 {
    match wired_limit_mb.filter(|mb| *mb > 0) {
        Some(mb) => mb.saturating_mul(MIB).min(total_ram_bytes),
        None => total_ram_bytes / 100 * APPLE_GPU_SHARE_PERCENT,
    }
}

/// What is kept free of the budget for prefill buffers and activations.
pub(crate) fn prefill_margin_bytes(budget_bytes: u64) -> u64 {
    (budget_bytes / 100 * MARGIN_SHARE_PERCENT).max(MARGIN_FLOOR_BYTES)
}

/// Tokens of cache that fit in what the budget leaves after the weights and the
/// margin. Zero when the weights alone leave nothing, which the caller raises to
/// the floor: a model the machine cannot hold with any window must say so
/// rather than be given a band it cannot honour.
pub(crate) fn window_within_gpu_budget(
    budget_bytes: u64,
    weights_bytes: u64,
    kv_bytes_per_token: u64,
) -> u64 {
    budget_bytes
        .saturating_sub(weights_bytes)
        .saturating_sub(prefill_margin_bytes(budget_bytes))
        / kv_bytes_per_token.max(1)
}

/// Where a ceiling came from, as far as the origin shown to the user goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CeilingBasis {
    /// The coarse band of installed memory — nothing was known of the model.
    MemoryBand,
    /// Computed for this model from its weights and the cost of its cache.
    ModelEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ModelCeiling {
    pub tokens: u64,
    pub basis: CeilingBasis,
}

impl From<u64> for ModelCeiling {
    fn from(tokens: u64) -> Self {
        Self {
            tokens,
            basis: CeilingBasis::MemoryBand,
        }
    }
}

/// Everything the ceiling of one model on one machine depends on. An absent
/// field is a fact Kronn does not have.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CeilingInputs {
    pub total_ram_bytes: Option<u64>,
    /// Set only where the GPU shares the machine's memory and its limit is
    /// readable (Apple Silicon). Elsewhere the budget is not RAM and the
    /// pre-KT-943 rule stands.
    pub gpu_budget_bytes: Option<u64>,
    pub weights_bytes: Option<u64>,
    /// Slope observed on the running server between two windows, when known.
    pub measured_kv_bytes_per_token: Option<u64>,
    pub shape: Option<KvShape>,
    /// `None` when `OLLAMA_KV_CACHE_TYPE` holds a value Kronn cannot price.
    pub kv_cache_type: Option<KvCacheType>,
}

/// The ceiling for one model. On Apple Silicon: `(GPU budget − weights −
/// margin) / KV bytes per token`, the cost per token being the one measured on
/// the server if there is one, else the one computed from the model's own
/// attention shape and the configured cache type. Elsewhere, and whenever an
/// input is missing: what Kronn did before, a measured slope over a share of RAM
/// or the memory band.
pub(crate) fn ceiling_for_model(inputs: &CeilingInputs) -> ModelCeiling {
    use super::runner::{
        ram_ceiling_for_model, ram_derived_ceiling, window_the_machine_can_hold,
        OLLAMA_NUM_CTX_FLOOR,
    };
    let band = || ModelCeiling::from(ram_derived_ceiling(inputs.total_ram_bytes));
    let Some(budget) = inputs.gpu_budget_bytes else {
        let exact = window_the_machine_can_hold(
            inputs.total_ram_bytes,
            inputs.weights_bytes,
            inputs.measured_kv_bytes_per_token,
        );
        return ModelCeiling {
            tokens: ram_ceiling_for_model(
                inputs.total_ram_bytes,
                inputs.weights_bytes,
                inputs.measured_kv_bytes_per_token,
            ),
            basis: if exact.is_some() {
                CeilingBasis::ModelEstimate
            } else {
                CeilingBasis::MemoryBand
            },
        };
    };
    let Some(weights) = inputs.weights_bytes else {
        return band();
    };
    let per_token = inputs
        .measured_kv_bytes_per_token
        .filter(|value| *value > 0)
        .or_else(|| Some(inputs.shape?.bytes_per_token(inputs.kv_cache_type?)));
    let Some(per_token) = per_token else {
        return band();
    };
    ModelCeiling {
        tokens: window_within_gpu_budget(budget, weights, per_token).max(OLLAMA_NUM_CTX_FLOOR),
        basis: CeilingBasis::ModelEstimate,
    }
}

// ── Reading the machine ─────────────────────────────────────────────────────

/// A variable of the Ollama server, as far as Kronn can see it: its own
/// environment first (a server started from the same shell), then the macOS
/// launchd environment where the Ollama app reads it from.
fn server_env(name: &str) -> Option<String> {
    let set = |raw: String| Some(raw.trim().to_string()).filter(|value| !value.is_empty());
    if let Some(value) = std::env::var(name).ok().and_then(set) {
        return Some(value);
    }
    #[cfg(target_os = "macos")]
    {
        let output = crate::core::cmd::sync_cmd("launchctl")
            .args(["getenv", name])
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok().and_then(set)
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Machine facts read once per decision rather than per model.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MachineFacts {
    pub total_ram_bytes: Option<u64>,
    pub gpu_budget_bytes: Option<u64>,
    pub kv_cache_type: Option<KvCacheType>,
}

impl MachineFacts {
    pub(crate) fn read() -> Self {
        let total_ram_bytes = super::runner::total_system_memory_bytes();
        Self {
            total_ram_bytes,
            gpu_budget_bytes: apple_silicon_gpu_budget(total_ram_bytes),
            kv_cache_type: KvCacheType::from_setting(server_env("OLLAMA_KV_CACHE_TYPE").as_deref()),
        }
    }
}

pub(crate) fn server_is_local(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The GPU budget on a Mac with Apple Silicon running Kronn natively. A Kronn in
/// a Linux container sees the container VM's memory, not the Mac's, so it keeps
/// the old rule rather than size a window against the wrong machine.
fn apple_silicon_gpu_budget(total_ram_bytes: Option<u64>) -> Option<u64> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let wired_limit_mb = crate::core::cmd::sync_cmd("sysctl")
            .args(["-n", "iogpu.wired_limit_mb"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|raw| raw.trim().parse().ok());
        Some(gpu_budget_bytes(total_ram_bytes?, wired_limit_mb))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = total_ram_bytes;
        None
    }
}

// ── Reading the model's own metadata from Ollama's store ────────────────────

/// Where Ollama keeps its manifests and blobs, most specific first: the
/// directory the server was told to use, then the per-user default, then the
/// Linux service account's.
fn model_stores() -> Vec<PathBuf> {
    let mut stores: Vec<PathBuf> = Vec::new();
    let mut add = |path: PathBuf| {
        if !stores.contains(&path) {
            stores.push(path);
        }
    };
    if let Some(dir) = server_env("OLLAMA_MODELS") {
        add(PathBuf::from(dir));
    }
    for variable in ["HOME", "KRONN_HOST_HOME"] {
        if let Some(home) = std::env::var_os(variable).filter(|home| !home.is_empty()) {
            add(PathBuf::from(home).join(".ollama").join("models"));
        }
    }
    #[cfg(target_os = "linux")]
    add(PathBuf::from("/usr/share/ollama/.ollama/models"));
    stores
}

/// `qwen3.8:27b-mlx` → `registry.ollama.ai/library/qwen3.8/27b-mlx`: where the
/// store files a tag's manifest. The name arrives from an HTTP request, so each
/// segment is checked to be a plain name before it becomes part of a path.
fn manifest_paths(model: &str) -> Vec<PathBuf> {
    let model = model.trim();
    let (name, tag) = match model.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => (name, tag),
        _ => (model, "latest"),
    };
    let plain = |segment: &str| {
        !matches!(segment, "" | "." | "..")
            && segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    let segments: Vec<&str> = name.split('/').collect();
    let [host, namespace, model_name] = match segments[..] {
        [model_name] => ["registry.ollama.ai", "library", model_name],
        [namespace, model_name] => ["registry.ollama.ai", namespace, model_name],
        [host, namespace, model_name] => [host, namespace, model_name],
        _ => return Vec::new(),
    };
    let parts = [host, namespace, model_name, tag];
    if !parts.iter().all(|part| plain(part)) {
        return Vec::new();
    }
    // The store lowercases what it files; try the name as written first.
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in [
        parts.iter().collect::<PathBuf>(),
        parts.iter().map(|part| part.to_ascii_lowercase()).collect(),
    ] {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

fn read_json_bounded(path: &Path, max_bytes: u64) -> Option<Value> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > max_bytes {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > max_bytes {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// The blob file name of the manifest's `config.json` layer
/// (`sha256:<64 hex>` is filed as `sha256-<64 hex>`).
fn config_blob_name(manifest: &Value) -> Option<String> {
    let digest = manifest
        .get("layers")?
        .as_array()?
        .iter()
        .find(|layer| layer.get("name").and_then(Value::as_str) == Some("config.json"))?
        .get("digest")?
        .as_str()?;
    let hex = digest.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("sha256-{hex}"))
}

/// The `config.json` an imported safetensors model carries in its manifest,
/// read from the first store that has the tag. `None` for a GGUF model (it has
/// no such layer), for a store this process cannot read — Kronn in a container
/// does not see the host's `~/.ollama` — and for anything unreadable.
pub(crate) fn read_model_config(stores: &[PathBuf], model: &str) -> Option<Value> {
    let relatives = manifest_paths(model);
    for store in stores {
        for relative in &relatives {
            let Some(manifest) =
                read_json_bounded(&store.join("manifests").join(relative), MAX_MANIFEST_BYTES)
            else {
                continue;
            };
            let blob = config_blob_name(&manifest)?;
            return read_json_bounded(&store.join("blobs").join(blob), MAX_CONFIG_BYTES);
        }
    }
    None
}

/// `kv_shape_from_config` of the model's own `config.json`, off the async
/// executor since it is file I/O.
pub(crate) async fn kv_shape_from_store(model: &str) -> Option<KvShape> {
    let model = model.to_string();
    tokio::task::spawn_blocking(move || {
        kv_shape_from_config(&read_model_config(&model_stores(), &model)?)
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
#[path = "ollama_memory_test.rs"]
mod tests;
