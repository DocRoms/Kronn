//! KT-930 — what the official Ollama library says about a model, without
//! downloading it.
//!
//! The registry serves every tag's manifest at
//! `https://registry.ollama.ai/v2/library/<name>/manifests/<tag>` (asked for
//! as `application/vnd.docker.distribution.manifest.v2+json`). Two facts come
//! out of that one small document:
//!
//! - **Freshness.** The registry sends no `Docker-Content-Digest` header, but
//!   the SHA-256 of the manifest body is exactly the `digest` a local Ollama
//!   reports for the same tag in `/api/tags` (checked on 2026-10-01 on the
//!   user's Mac: `qwen3.8:27b-mlx` and `qwen3.5:4b` identical, `gemma4:12b-mlx`
//!   different, a real pending update). So "is there an update" is a hash
//!   comparison, not a download.
//! - **Size.** The config's size plus every layer's size is what a pull of the
//!   tag transfers.
//!
//! What this module never does: guess. A registry that did not answer, a model
//! outside the official library (a namespace, a Hugging Face reference), a
//! body that is not a manifest, a digest that cannot be read — each is
//! `Unknown`, never "up to date".
//!
//! Requests are small, bounded in time, size and parallelism, and answers are
//! cached for hours, so opening Settings costs the registry (and the user)
//! next to nothing. The manifest source is a trait so the tests exercise every
//! branch without a network or a socket.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::models::{
    OllamaModelFreshness, OllamaRegistryResponse, OllamaSuggestionSize, OllamaUpdateStatus,
};

/// Where the official library lives.
pub const REGISTRY_BASE: &str = "https://registry.ollama.ai";
const MANIFEST_ACCEPT: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// Per request. A manifest is a couple of kilobytes: a registry that needs
/// longer than this is not one worth waiting for.
const FETCH_TIMEOUT: Duration = Duration::from_secs(4);
/// What one whole answer may take, however many tags it covers. Whatever
/// finished by then is used and cached; the rest stays unknown until the next
/// look.
pub const BATCH_BUDGET: Duration = Duration::from_secs(8);
/// A real manifest is under 2 KB; this only guards against a hostile or
/// broken endpoint streaming something enormous.
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_CONCURRENT_LOOKUPS: usize = 4;
/// A tag moves rarely; hours-old is fine for a badge.
const FOUND_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// A failure is remembered just long enough that a blocked network does not
/// make every visit to Settings wait out the timeouts again.
const FAILED_TTL: Duration = Duration::from_secs(2 * 60);
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_NAME_LEN: usize = 128;
/// How many tags one request may ask about: the installed models plus the
/// suggestions, with room to spare, never an open-ended fan-out.
pub const MAX_LOOKUPS: usize = 48;

/// A model of the official library: `library/<family>:<tag>`. Parsing is the
/// gate for every outbound request, so only names that can be nothing else
/// than a library path segment get through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRef {
    family: String,
    tag: String,
}

fn valid_family(family: &str) -> bool {
    let mut chars = family.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
}

fn valid_tag(tag: &str) -> bool {
    let mut chars = tag.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl LibraryRef {
    /// `qwen3:8b` → the library's `qwen3` at tag `8b`; `qwen3` → `:latest`, as
    /// Ollama itself reads it. `None` for anything that is not a plain
    /// library name: a namespace (`user/model`), a host (`hf.co/…`), a second
    /// colon, an empty part, an uppercase family, a character outside the
    /// ones Ollama names are made of.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.is_empty() || name.len() > MAX_NAME_LEN {
            return None;
        }
        let (family, tag) = name.split_once(':').unwrap_or((name, "latest"));
        (valid_family(family) && valid_tag(tag)).then(|| Self {
            family: family.to_owned(),
            tag: tag.to_owned(),
        })
    }

    /// The cache key, and how a model is found again in an answer.
    pub fn key(&self) -> String {
        format!("{}:{}", self.family, self.tag)
    }

    fn manifest_url(&self, base: &str) -> String {
        format!(
            "{}/v2/library/{}/manifests/{}",
            base.trim_end_matches('/'),
            self.family,
            self.tag
        )
    }
}

/// What a manifest tells Kronn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// SHA-256 of the manifest body exactly as the registry sent it, lower-case
    /// hex. Comparable with the `digest` of a local `/api/tags` entry.
    pub digest: String,
    /// Config plus layers, in bytes. `None` when a size is missing or the sum
    /// does not fit: a wrong total is worse than none.
    pub size_bytes: Option<u64>,
}

#[derive(Deserialize)]
struct ManifestWire {
    config: Option<BlobWire>,
    layers: Option<Vec<BlobWire>>,
}

#[derive(Deserialize)]
struct BlobWire {
    size: Option<u64>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Reads a manifest body. The digest is taken over `body` as received, never
/// over a re-serialisation: whitespace and key order are part of what the
/// registry (and a local Ollama) hashed. `None` for a body that is not a
/// manifest — an HTML error page served with a 200 must not be hashed into a
/// verdict.
pub fn parse_manifest(body: &[u8]) -> Option<Manifest> {
    let wire: ManifestWire = serde_json::from_slice(body).ok()?;
    let layers = wire.layers.filter(|layers| !layers.is_empty())?;
    let size_bytes = wire
        .config
        .into_iter()
        .chain(layers)
        .try_fold(0u64, |total, blob| total.checked_add(blob.size?));
    Some(Manifest {
        digest: sha256_hex(body),
        size_bytes,
    })
}

/// A digest as `/api/tags` or the registry writes it — bare hex or
/// `sha256:`-prefixed, any case — or nothing.
fn normalize_digest(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let hex = raw
        .strip_prefix("sha256:")
        .unwrap_or(raw)
        .to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(hex)
}

/// The verdict for one installed model. Only two readable digests that match
/// are "up to date"; everything short of that is `Unknown` or an update.
pub fn update_status(local_digest: &str, registry: Option<&Manifest>) -> OllamaUpdateStatus {
    let (Some(local), Some(registry)) = (normalize_digest(local_digest), registry) else {
        return OllamaUpdateStatus::Unknown;
    };
    if local == registry.digest {
        OllamaUpdateStatus::UpToDate
    } else {
        OllamaUpdateStatus::UpdateAvailable
    }
}

/// Where manifests come from.
#[async_trait]
pub trait ManifestSource: Send + Sync {
    /// The manifest body exactly as served, or `None` for anything that is not
    /// a usable answer (transport failure, timeout, non-200, oversized).
    async fn manifest(&self, model: &LibraryRef) -> Option<Vec<u8>>;
}

/// The real thing: HTTPS to the registry.
pub struct HttpManifestSource {
    client: reqwest::Client,
    base: String,
}

impl HttpManifestSource {
    pub fn new(base: impl Into<String>) -> Self {
        Self::with_client(
            reqwest::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base,
        )
    }

    pub fn with_client(client: reqwest::Client, base: impl Into<String>) -> Self {
        Self {
            client,
            base: base.into(),
        }
    }
}

#[async_trait]
impl ManifestSource for HttpManifestSource {
    async fn manifest(&self, model: &LibraryRef) -> Option<Vec<u8>> {
        let response = self
            .client
            .get(model.manifest_url(&self.base))
            .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT)
            .send()
            .await
            .ok()?;
        if response.status() != reqwest::StatusCode::OK {
            return None;
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
        {
            return None;
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            if body.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
                return None;
            }
            body.extend_from_slice(&chunk);
        }
        Some(body)
    }
}

struct CacheEntry {
    at: Instant,
    manifest: Option<Manifest>,
}

impl CacheEntry {
    fn fresh_at(&self, now: Instant) -> bool {
        let ttl = if self.manifest.is_some() {
            FOUND_TTL
        } else {
            FAILED_TTL
        };
        now.saturating_duration_since(self.at) < ttl
    }
}

/// The manifest lookups of the official library, cached.
pub struct OllamaRegistry {
    source: Arc<dyn ManifestSource>,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl OllamaRegistry {
    pub fn new(source: Arc<dyn ManifestSource>) -> Self {
        Self {
            source,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cached(&self, key: &str, now: Instant) -> Option<Option<Manifest>> {
        let cache = self.cache.lock().ok()?;
        cache
            .get(key)
            .filter(|entry| entry.fresh_at(now))
            .map(|entry| entry.manifest.clone())
    }

    fn remember(&self, key: String, manifest: Option<Manifest>, now: Instant) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.retain(|_, entry| entry.fresh_at(now));
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(key, CacheEntry { at: now, manifest });
    }

    /// One tag's manifest: from the cache while it is fresh, else from the
    /// source. A failure is cached too, briefly, as `None`.
    pub async fn lookup_at(&self, model: &LibraryRef, now: Instant) -> Option<Manifest> {
        let key = model.key();
        if let Some(hit) = self.cached(&key, now) {
            return hit;
        }
        let manifest = self
            .source
            .manifest(model)
            .await
            .and_then(|body| parse_manifest(&body));
        self.remember(key, manifest.clone(), now);
        manifest
    }

    pub async fn lookup(&self, model: &LibraryRef) -> Option<Manifest> {
        self.lookup_at(model, Instant::now()).await
    }

    /// Several tags at once, [`MAX_CONCURRENT_LOOKUPS`] at a time, within
    /// `budget` overall. A tag missing from the result did not answer in time.
    /// Lookups finished by the deadline are already cached, so asking again
    /// resumes instead of starting over.
    pub async fn lookup_all(
        &self,
        models: &[LibraryRef],
        budget: Duration,
    ) -> HashMap<String, Option<Manifest>> {
        let deadline = tokio::time::Instant::now() + budget;
        let mut answers = HashMap::new();
        // Owned items: a stream of borrowed ones makes this future's `Send`
        // unprovable once it sits behind an axum handler.
        let mut pending = futures::stream::iter(models.to_vec())
            .map(|model| async move {
                let manifest = self.lookup(&model).await;
                (model.key(), manifest)
            })
            .buffer_unordered(MAX_CONCURRENT_LOOKUPS);
        while let Ok(Some((key, manifest))) =
            tokio::time::timeout_at(deadline, pending.next()).await
        {
            answers.insert(key, manifest);
        }
        answers
    }
}

/// The process-wide registry, asking the real one.
pub fn shared() -> &'static OllamaRegistry {
    static REGISTRY: OnceLock<OllamaRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| OllamaRegistry::new(Arc::new(HttpManifestSource::new(REGISTRY_BASE))))
}

/// The answer for `GET /api/ollama/registry`: every installed model's
/// freshness and every suggested tag's size, from at most one lookup per
/// distinct library tag. `installed` is `(name, local digest)`.
pub async fn report(
    registry: &OllamaRegistry,
    installed: &[(String, String)],
    suggested: &[String],
    budget: Duration,
) -> OllamaRegistryResponse {
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    let names = installed
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(suggested.iter().map(String::as_str));
    for model in names.filter_map(LibraryRef::parse) {
        if refs.len() < MAX_LOOKUPS && seen.insert(model.key()) {
            refs.push(model);
        }
    }
    let answers = registry.lookup_all(&refs, budget).await;
    let manifest_of = |name: &str| -> Option<&Manifest> {
        let model = LibraryRef::parse(name)?;
        answers.get(&model.key())?.as_ref()
    };

    let models = installed
        .iter()
        .map(|(name, digest)| OllamaModelFreshness {
            name: name.clone(),
            status: update_status(digest, manifest_of(name)),
        })
        .collect();

    let mut sized = HashSet::new();
    let suggestions = suggested
        .iter()
        .filter(|name| sized.insert(name.as_str()))
        .filter_map(|name| {
            let bytes = manifest_of(name)?.size_bytes?;
            Some(OllamaSuggestionSize {
                name: name.clone(),
                size: crate::api::ollama::format_size(bytes),
            })
        })
        .collect();

    OllamaRegistryResponse {
        models,
        suggestions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A manifest in the shape the registry serves (Docker image manifest v2:
    /// a config blob and the model, template and params layers), compact as the
    /// registry sends it. Built by hand — with fake blob digests — so the
    /// tests need no network; the digest asserted below was computed
    /// independently with `shasum -a 256` over exactly these bytes.
    const FIXTURE: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","size":487},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:2222222222222222222222222222222222222222222222222222222222222222","size":2500000000},{"mediaType":"application/vnd.ollama.image.template","digest":"sha256:3333333333333333333333333333333333333333333333333333333333333333","size":1400},{"mediaType":"application/vnd.ollama.image.params","digest":"sha256:4444444444444444444444444444444444444444444444444444444444444444","size":120}]}"#;
    const FIXTURE_DIGEST: &str = "7c8a423ee36b7b1b2588d3d4fb5c76226e899cd52c4140d87fd5e0a52a2a9755";
    /// The same JSON with different whitespace: same content, other bytes.
    const FIXTURE_REWRAPPED: &str = "{\"schemaVersion\": 2,\n \"config\": {\"size\": 487},\n \"layers\": [{\"size\": 2500000000}, {\"size\": 1400}, {\"size\": 120}]}";

    /// A second tag whose manifest differs (one layer moved on).
    fn newer_fixture() -> String {
        FIXTURE.replace("\"size\":1400", "\"size\":1500")
    }

    fn model(name: &str) -> LibraryRef {
        LibraryRef::parse(name).unwrap_or_else(|| panic!("{name} is a library name"))
    }

    // ─── names ───────────────────────────────────────────────────────────

    #[test]
    fn only_plain_library_names_become_a_request() {
        let ok = |name: &str| LibraryRef::parse(name).map(|m| m.key());
        assert_eq!(ok("qwen3:8b").as_deref(), Some("qwen3:8b"));
        assert_eq!(ok("qwen3.5:4b").as_deref(), Some("qwen3.5:4b"));
        assert_eq!(ok("gemma4:12b-mlx").as_deref(), Some("gemma4:12b-mlx"));
        assert_eq!(ok("qwen3.8:27b-mlx").as_deref(), Some("qwen3.8:27b-mlx"));
        assert_eq!(
            ok("llama3.3:70b-instruct-q4_K_M").as_deref(),
            Some("llama3.3:70b-instruct-q4_K_M")
        );
        assert_eq!(ok("  qwen3:8b ").as_deref(), Some("qwen3:8b"), "trimmed");
        assert_eq!(
            ok("qwen3").as_deref(),
            Some("qwen3:latest"),
            "no tag is :latest"
        );

        for outside in [
            "",
            "   ",
            "hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M",
            "someone/their-model:latest",
            "library/qwen3:8b",
            "registry.ollama.ai/library/qwen3:8b",
            "Qwen3:8b",
            "qwen3:",
            ":8b",
            "qwen3:8b:extra",
            "qwen3:8 b",
            "qwen3:8b/../x",
            "..:8b",
            "-qwen3:8b",
            "qwen3:-8b",
            "qwen3:8b?x=1",
            "qwen3:8b#frag",
        ] {
            assert_eq!(LibraryRef::parse(outside), None, "{outside:?}");
        }
        assert_eq!(LibraryRef::parse(&"a".repeat(MAX_NAME_LEN + 1)), None);
    }

    #[test]
    fn the_manifest_url_is_the_library_path_of_the_tag() {
        assert_eq!(
            model("qwen3.8:27b-mlx").manifest_url("https://registry.ollama.ai"),
            "https://registry.ollama.ai/v2/library/qwen3.8/manifests/27b-mlx"
        );
        assert_eq!(
            model("qwen3").manifest_url("http://localhost:1234/"),
            "http://localhost:1234/v2/library/qwen3/manifests/latest"
        );
    }

    // ─── manifest ────────────────────────────────────────────────────────

    #[test]
    fn sha256_matches_the_known_answer() {
        // FIPS 180-2 test vector for "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_digest_is_the_sha256_of_the_body_as_served() {
        let manifest = parse_manifest(FIXTURE.as_bytes()).expect("a manifest");
        assert_eq!(manifest.digest, FIXTURE_DIGEST);

        // Same JSON content, different bytes: a different digest. The body is
        // hashed as received, never re-serialised.
        let rewrapped = parse_manifest(FIXTURE_REWRAPPED.as_bytes()).expect("a manifest");
        assert_ne!(rewrapped.digest, manifest.digest);
        assert_eq!(rewrapped.size_bytes, manifest.size_bytes);
    }

    #[test]
    fn the_size_is_the_config_plus_every_layer() {
        let manifest = parse_manifest(FIXTURE.as_bytes()).expect("a manifest");
        assert_eq!(manifest.size_bytes, Some(487 + 2_500_000_000 + 1_400 + 120));
    }

    #[test]
    fn a_size_that_cannot_be_trusted_is_none_not_a_partial_sum() {
        let missing = r#"{"layers":[{"size":10},{"digest":"sha256:x"}]}"#;
        let manifest = parse_manifest(missing.as_bytes()).expect("still a manifest");
        assert_eq!(manifest.size_bytes, None);
        assert_eq!(
            manifest.digest,
            sha256_hex(missing.as_bytes()),
            "the digest stands"
        );

        let overflow = format!(r#"{{"layers":[{{"size":{}}},{{"size":1}}]}}"#, u64::MAX);
        assert_eq!(
            parse_manifest(overflow.as_bytes())
                .expect("manifest")
                .size_bytes,
            None
        );
    }

    #[test]
    fn a_body_that_is_not_a_manifest_is_not_hashed_into_a_verdict() {
        for body in [
            "",
            "<html><body>502 Bad Gateway</body></html>",
            "null",
            "[]",
            "{}",
            r#"{"layers":[]}"#,
            r#"{"errors":[{"code":"MANIFEST_UNKNOWN"}]}"#,
            r#"{"layers":"nope"}"#,
        ] {
            assert_eq!(parse_manifest(body.as_bytes()), None, "{body:?}");
        }
    }

    // ─── comparison ──────────────────────────────────────────────────────

    #[test]
    fn identical_digests_are_up_to_date() {
        let registry = parse_manifest(FIXTURE.as_bytes()).unwrap();
        assert_eq!(
            update_status(FIXTURE_DIGEST, Some(&registry)),
            OllamaUpdateStatus::UpToDate
        );
        // As /api/tags or a registry client may write the same digest.
        assert_eq!(
            update_status(&format!("sha256:{FIXTURE_DIGEST}"), Some(&registry)),
            OllamaUpdateStatus::UpToDate
        );
        assert_eq!(
            update_status(
                &format!(" {} ", FIXTURE_DIGEST.to_uppercase()),
                Some(&registry)
            ),
            OllamaUpdateStatus::UpToDate
        );
    }

    #[test]
    fn different_digests_are_an_update() {
        let registry = parse_manifest(newer_fixture().as_bytes()).unwrap();
        assert_eq!(
            update_status(FIXTURE_DIGEST, Some(&registry)),
            OllamaUpdateStatus::UpdateAvailable
        );
    }

    #[test]
    fn no_registry_answer_is_unknown_never_up_to_date() {
        assert_eq!(
            update_status(FIXTURE_DIGEST, None),
            OllamaUpdateStatus::Unknown
        );
    }

    #[test]
    fn an_unreadable_local_digest_is_unknown_never_up_to_date() {
        let registry = parse_manifest(FIXTURE.as_bytes()).unwrap();
        for local in [
            "",
            "   ",
            "sha256:",
            // A prefix of the right digest is not the digest.
            &FIXTURE_DIGEST[..16],
            "not-a-digest",
            &"g".repeat(64),
            &format!("{FIXTURE_DIGEST}00"),
        ] {
            assert_eq!(
                update_status(local, Some(&registry)),
                OllamaUpdateStatus::Unknown,
                "{local:?}"
            );
        }
    }

    // ─── lookups, with a scripted source ─────────────────────────────────

    /// Answers from a script, counts what it is asked, and can be slow or
    /// never answer.
    #[derive(Default)]
    struct FakeSource {
        bodies: Mutex<HashMap<String, Vec<u8>>>,
        asked: Mutex<Vec<String>>,
        delay: Duration,
        hang_on: Option<String>,
        in_flight: AtomicUsize,
        max_in_flight: AtomicUsize,
    }

    impl FakeSource {
        fn serving(entries: &[(&str, &str)]) -> Self {
            let source = Self::default();
            for (name, body) in entries {
                source.set(name, body);
            }
            source
        }
        fn set(&self, name: &str, body: &str) {
            self.bodies
                .lock()
                .unwrap()
                .insert(model(name).key(), body.as_bytes().to_vec());
        }
        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl ManifestSource for FakeSource {
        async fn manifest(&self, model: &LibraryRef) -> Option<Vec<u8>> {
            self.asked.lock().unwrap().push(model.key());
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_in_flight.fetch_max(now, Ordering::SeqCst);
            if self.hang_on.as_deref() == Some(&model.key()) {
                futures::future::pending::<()>().await;
            }
            tokio::time::sleep(self.delay).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            self.bodies.lock().unwrap().get(&model.key()).cloned()
        }
    }

    fn registry_of(source: Arc<FakeSource>) -> OllamaRegistry {
        OllamaRegistry::new(source)
    }

    fn installed(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(name, digest)| (name.to_string(), digest.to_string()))
            .collect()
    }

    fn status_of(response: &OllamaRegistryResponse, name: &str) -> OllamaUpdateStatus {
        response
            .models
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{name} is in the answer"))
            .status
    }

    #[tokio::test]
    async fn a_model_the_library_still_serves_unchanged_is_up_to_date() {
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", FIXTURE)]));
        let response = report(
            &registry_of(source),
            &installed(&[("qwen3:8b", FIXTURE_DIGEST)]),
            &[],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::UpToDate
        );
    }

    #[tokio::test]
    async fn a_model_whose_tag_moved_on_has_an_update() {
        let newer = newer_fixture();
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", newer.as_str())]));
        let response = report(
            &registry_of(source),
            &installed(&[("qwen3:8b", FIXTURE_DIGEST)]),
            &[],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::UpdateAvailable
        );
    }

    #[tokio::test]
    async fn a_registry_that_does_not_answer_leaves_every_model_unknown() {
        // Nothing scripted: every lookup fails, as a dead network would.
        let source = Arc::new(FakeSource::default());
        let response = report(
            &registry_of(source),
            &installed(&[
                ("qwen3:8b", FIXTURE_DIGEST),
                ("gemma4:12b-mlx", FIXTURE_DIGEST),
            ]),
            &["qwen3.5:4b".to_string()],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::Unknown
        );
        assert_eq!(
            status_of(&response, "gemma4:12b-mlx"),
            OllamaUpdateStatus::Unknown
        );
        assert!(
            response.suggestions.is_empty(),
            "no size is invented either"
        );
    }

    #[tokio::test]
    async fn a_model_outside_the_library_is_unknown_and_never_asked_about() {
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", FIXTURE)]));
        let response = report(
            &registry_of(source.clone()),
            &installed(&[
                (
                    "hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M",
                    FIXTURE_DIGEST,
                ),
                ("someone/their-model:latest", FIXTURE_DIGEST),
                ("qwen3:8b", FIXTURE_DIGEST),
            ]),
            &["../../etc/passwd".to_string(), "a b".to_string()],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(
                &response,
                "hf.co/bartowski/Llama-3.2-3B-Instruct-GGUF:Q4_K_M"
            ),
            OllamaUpdateStatus::Unknown
        );
        assert_eq!(
            status_of(&response, "someone/their-model:latest"),
            OllamaUpdateStatus::Unknown
        );
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::UpToDate
        );
        assert_eq!(
            source.asked(),
            vec!["qwen3:8b"],
            "only the library name left the process"
        );
        assert!(response.suggestions.is_empty());
    }

    #[tokio::test]
    async fn a_local_model_without_a_readable_digest_is_unknown_even_when_the_library_answers() {
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", FIXTURE)]));
        let response = report(
            &registry_of(source),
            &installed(&[("qwen3:8b", "")]),
            &[],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::Unknown
        );
    }

    #[tokio::test]
    async fn suggestions_carry_the_size_the_manifest_adds_up_to() {
        let source = Arc::new(FakeSource::serving(&[("qwen3.5:4b", FIXTURE)]));
        let response = report(
            &registry_of(source),
            &[],
            &[
                "qwen3.5:4b".to_string(),
                // The library does not answer for this one: no size, no guess.
                "qwen3:8b".to_string(),
                // Asked twice, answered once.
                "qwen3.5:4b".to_string(),
            ],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(response.suggestions.len(), 1, "{:?}", response.suggestions);
        assert_eq!(response.suggestions[0].name, "qwen3.5:4b");
        assert_eq!(response.suggestions[0].size, "2.5 GB");
    }

    #[tokio::test]
    async fn an_installed_suggestion_costs_one_lookup_and_answers_both_questions() {
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", FIXTURE)]));
        let response = report(
            &registry_of(source.clone()),
            &installed(&[("qwen3:8b", FIXTURE_DIGEST)]),
            &["qwen3:8b".to_string()],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(source.asked().len(), 1);
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::UpToDate
        );
        assert_eq!(response.suggestions.len(), 1);
    }

    #[tokio::test]
    async fn answers_are_cached_for_hours_and_then_asked_for_again() {
        let source = Arc::new(FakeSource::serving(&[("qwen3:8b", FIXTURE)]));
        let registry = registry_of(source.clone());
        let t0 = Instant::now();
        let qwen = model("qwen3:8b");

        assert!(registry.lookup_at(&qwen, t0).await.is_some());
        assert!(registry
            .lookup_at(&qwen, t0 + Duration::from_secs(3600))
            .await
            .is_some());
        assert!(registry
            .lookup_at(&qwen, t0 + FOUND_TTL - Duration::from_secs(1))
            .await
            .is_some());
        assert_eq!(
            source.asked().len(),
            1,
            "an hour later is still the first answer"
        );

        // The tag moved on while the cache held the old answer.
        let newer = newer_fixture();
        source.set("qwen3:8b", &newer);
        let refreshed = registry
            .lookup_at(&qwen, t0 + FOUND_TTL + Duration::from_secs(1))
            .await
            .expect("answered");
        assert_eq!(source.asked().len(), 2);
        assert_eq!(refreshed.digest, sha256_hex(newer.as_bytes()));
    }

    #[tokio::test]
    async fn a_failure_is_remembered_briefly_then_retried() {
        let source = Arc::new(FakeSource::default());
        let registry = registry_of(source.clone());
        let t0 = Instant::now();
        let qwen = model("qwen3:8b");

        assert!(registry.lookup_at(&qwen, t0).await.is_none());
        assert!(registry
            .lookup_at(&qwen, t0 + Duration::from_secs(30))
            .await
            .is_none());
        assert_eq!(source.asked().len(), 1, "a blocked network is not hammered");

        // The network is back.
        source.set("qwen3:8b", FIXTURE);
        let recovered = registry
            .lookup_at(&qwen, t0 + FAILED_TTL + Duration::from_secs(1))
            .await;
        assert!(recovered.is_some(), "a failure never sticks for hours");
        assert_eq!(source.asked().len(), 2);
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_manifest_counts_as_no_answer() {
        let source = Arc::new(FakeSource::serving(&[(
            "qwen3:8b",
            "<html>captive portal</html>",
        )]));
        let response = report(
            &registry_of(source),
            &installed(&[("qwen3:8b", FIXTURE_DIGEST)]),
            &["qwen3:8b".to_string()],
            BATCH_BUDGET,
        )
        .await;
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::Unknown
        );
        assert!(response.suggestions.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn lookups_run_a_few_at_a_time_never_all_at_once() {
        let names: Vec<String> = (0..12).map(|i| format!("model{i}:latest")).collect();
        let source = Arc::new(FakeSource {
            delay: Duration::from_millis(100),
            ..FakeSource::default()
        });
        for name in &names {
            source.set(name, FIXTURE);
        }
        let refs: Vec<LibraryRef> = names.iter().map(|n| model(n)).collect();

        let answers = registry_of(source.clone())
            .lookup_all(&refs, BATCH_BUDGET)
            .await;

        assert_eq!(answers.len(), 12);
        let peak = source.max_in_flight.load(Ordering::SeqCst);
        assert!(peak > 1, "they do overlap ({peak})");
        assert!(
            peak <= MAX_CONCURRENT_LOOKUPS,
            "at most {MAX_CONCURRENT_LOOKUPS} at a time, saw {peak}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn one_registry_that_never_answers_cannot_hold_the_page_past_the_budget() {
        let source = Arc::new(FakeSource {
            hang_on: Some("qwen3:30b-a3b".into()),
            ..FakeSource::default()
        });
        source.set("qwen3:8b", FIXTURE);
        source.set("qwen3:30b-a3b", FIXTURE);
        let registry = registry_of(source);

        let started = tokio::time::Instant::now();
        let response = report(
            &registry,
            &installed(&[
                ("qwen3:8b", FIXTURE_DIGEST),
                ("qwen3:30b-a3b", FIXTURE_DIGEST),
            ]),
            &[],
            Duration::from_secs(2),
        )
        .await;

        assert!(started.elapsed() <= Duration::from_secs(2));
        assert_eq!(
            status_of(&response, "qwen3:8b"),
            OllamaUpdateStatus::UpToDate
        );
        assert_eq!(
            status_of(&response, "qwen3:30b-a3b"),
            OllamaUpdateStatus::Unknown,
            "late is unknown, not up to date"
        );
    }

    #[tokio::test]
    async fn a_request_cannot_fan_out_past_the_lookup_cap() {
        let source = Arc::new(FakeSource::default());
        let suggested: Vec<String> = (0..MAX_LOOKUPS * 2).map(|i| format!("m{i}:1")).collect();
        let _ = report(&registry_of(source.clone()), &[], &suggested, BATCH_BUDGET).await;
        assert_eq!(source.asked().len(), MAX_LOOKUPS);
    }

    /// The one place the registry is spoken to, against a local mock: the path
    /// and `Accept` header the registry expects, the body returned untouched
    /// (it is what gets hashed), and every non-answer as `None`. Needs a local
    /// socket for the mock server, which a sandbox may refuse.
    #[tokio::test]
    async fn the_http_source_asks_the_way_the_registry_serves_and_fails_closed() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v2/library/qwen3.5/manifests/4b"))
            .and(header("accept", MANIFEST_ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_raw(FIXTURE, MANIFEST_ACCEPT))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/library/broken/manifests/1"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/library/huge/manifests/1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAX_MANIFEST_BYTES + 1]),
            )
            .mount(&server)
            .await;

        let source = HttpManifestSource::with_client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(FETCH_TIMEOUT)
                .build()
                .unwrap(),
            server.uri(),
        );
        assert_eq!(
            source.manifest(&model("qwen3.5:4b")).await.as_deref(),
            Some(FIXTURE.as_bytes()),
            "the body, byte for byte"
        );
        assert_eq!(source.manifest(&model("missing:1")).await, None, "404");
        assert_eq!(source.manifest(&model("broken:1")).await, None, "500");
        assert_eq!(source.manifest(&model("huge:1")).await, None, "oversized");

        drop(server);
        assert_eq!(
            source.manifest(&model("qwen3.5:4b")).await,
            None,
            "nothing listening"
        );
    }

    #[test]
    fn the_cache_stays_bounded() {
        let registry = registry_of(Arc::new(FakeSource::default()));
        let now = Instant::now();
        for i in 0..MAX_CACHE_ENTRIES * 3 {
            registry.remember(format!("m{i}:1"), None, now);
        }
        assert!(registry.cache.lock().unwrap().len() <= MAX_CACHE_ENTRIES);
    }
}
