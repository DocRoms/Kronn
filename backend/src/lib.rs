pub mod api;
pub mod agents;
pub mod core;
pub mod db;
pub mod models;
pub mod workflows;

#[cfg(test)]
mod api_tests;

use std::sync::Arc;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    routing::{delete, get, patch, post, put},
    Router,
};
use tokio::sync::{RwLock, Semaphore};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tower_http::{
    cors::{CorsLayer, AllowOrigin},
    trace::TraceLayer,
};

pub use crate::db::Database;
pub use crate::models::AppConfig;
pub use crate::workflows::WorkflowEngine;

// ─── Application State ──────────────────────────────────────────────────────

/// Default maximum concurrent agent processes.
pub const DEFAULT_MAX_CONCURRENT_AGENTS: usize = 5;

/// Tracks running audit processes so they can be cancelled AND inspected.
///
/// `progress` is the data source for `GET /api/projects/:id/audit-status` —
/// the UI polls it to resume the progress bar after tab/page navigation.
/// Entries are inserted by the SSE streams (`full_audit`, `partial_audit`)
/// and removed on completion/cancel/error.
#[derive(Default)]
pub struct AuditTracker {
    /// Currently running child PID per project (if any)
    pub running_pids: HashMap<String, u32>,
    /// Projects whose audit should be cancelled
    pub cancelled: HashSet<String>,
    /// Live progress snapshot per project — empty when no audit runs.
    pub progress: HashMap<String, crate::models::AuditProgress>,
    /// Projects with an audit lease held. One audit (Full, specialized OR
    /// partial) per project at a time: `try_acquire_lease` is the single
    /// atomic gate all three launch paths go through, replacing the old
    /// check-then-insert race (two callers could both see the tracker + DB
    /// idle, then both insert a Running row). Released by the run's
    /// drop-guard when it ends — normally, on cancel, or on abandonment.
    pub leased: HashSet<String>,
}

impl AuditTracker {
    /// Atomically take the audit lease for a project. Returns `false` if one
    /// is already held (caller must refuse the launch). Called under the
    /// tracker mutex, so the check-and-insert can't race a second caller.
    pub fn try_acquire_lease(&mut self, project_id: &str) -> bool {
        self.leased.insert(project_id.to_string())
    }

    /// Release a project's audit lease. Idempotent.
    pub fn release_lease(&mut self, project_id: &str) {
        self.leased.remove(project_id);
    }

    /// Seed progress when an audit stream starts. Called from the `start`
    /// SSE event. Resets any stale progress row for the same project.
    pub fn start_progress(
        &mut self,
        project_id: impl Into<String>,
        total_steps: u32,
        kind: &str,
    ) {
        let project_id = project_id.into();
        self.progress.insert(
            project_id.clone(),
            crate::models::AuditProgress {
                project_id,
                phase: "auditing".into(),
                step_index: 0,
                total_steps,
                current_file: None,
                started_at: chrono::Utc::now(),
                kind: kind.into(),
                step_tokens: None,
                total_tokens_so_far: None,
                current_tool: None,
                current_tool_call_count: None,
            },
        );
    }

    /// 0.8.3 — update the live-chip state on every step_progress
    /// SSE event. The poll endpoint surfaces these fields so the
    /// frontend can re-seed the chips when SSE buffers / stalls.
    pub fn update_chips(
        &mut self,
        project_id: &str,
        step_tokens: Option<u64>,
        total_tokens_so_far: Option<u64>,
        current_tool: Option<String>,
    ) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            if let Some(s) = step_tokens { entry.step_tokens = Some(s); }
            if let Some(t) = total_tokens_so_far { entry.total_tokens_so_far = Some(t); }
            if let Some(tool) = current_tool {
                entry.current_tool = Some(tool);
                // 0.8.4 (#319 / B3) — bump the tool-call counter for
                // the live "still alive" chip. Every tool_call from
                // the agent stream lands here, regardless of whether
                // the agent also emitted a `Usage` block. Reset on
                // step boundary via `clear_step_chips`.
                entry.current_tool_call_count = Some(entry.current_tool_call_count.unwrap_or(0) + 1);
            }
        }
    }

    /// 0.8.3 — clear the per-step ephemeral chips when a step ends.
    /// Keeps `total_tokens_so_far` intact (it's cumulative across steps).
    pub fn clear_step_chips(&mut self, project_id: &str) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.step_tokens = None;
            entry.current_tool = None;
            // 0.8.4 (#319 / B3) — the tool-call counter is per-step,
            // not cumulative across the audit. Reset on every
            // `step_start` so chip reads `🔧 Tool (1)` then `(2)` etc.
            entry.current_tool_call_count = None;
        }
    }

    /// Update the step counter when a `step_start` SSE event fires. No-op
    /// if no progress row exists (the caller never invoked `start_progress`).
    pub fn advance_step(
        &mut self,
        project_id: &str,
        step_index: u32,
        current_file: Option<String>,
    ) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.step_index = step_index;
            entry.current_file = current_file;
            entry.phase = "auditing".into();
        }
    }

    /// Mark the audit as transitioning to phase 3 (validation discussion
    /// creation). Brief window — the caller clears progress right after.
    pub fn mark_validating(&mut self, project_id: &str) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.phase = "validating".into();
        }
    }

    /// Remove the progress entry for a project — called on `done`,
    /// `cancelled`, and fatal `step_error`.
    pub fn clear_progress(&mut self, project_id: &str) {
        self.progress.remove(project_id);
    }

    /// Read the current progress snapshot (cloned for safe release of the
    /// mutex). Used by the `audit-status` endpoint.
    pub fn get_progress(&self, project_id: &str) -> Option<crate::models::AuditProgress> {
        self.progress.get(project_id).cloned()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub db: Arc<Database>,
    pub agent_semaphore: Arc<Semaphore>,
    pub audit_tracker: Arc<Mutex<AuditTracker>>,
    /// Broadcast channel for WebSocket messages (presence, heartbeat, chat, batch progress).
    pub ws_broadcast: Arc<tokio::sync::broadcast::Sender<crate::models::WsMessage>>,
    /// Registry of in-flight cancellable tasks keyed by a string id — used by
    /// the "⏹ Arrêter" UI to abort a running agent discussion or workflow
    /// run. Keys are either a `disc_id` (from `make_agent_stream`) or a
    /// `run_id` (from the workflow runner). Tokens are inserted when work
    /// starts and removed in its finally-block — see the Registry impl below.
    pub cancel_registry: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// OAuth2 access-token cache for API plugins. Keyed by `mcp_configs.id`,
    /// value is the bearer token + its absolute expiry. In-memory only —
    /// on restart, tokens are lost and re-exchanged on first use (one HTTP
    /// call per active OAuth2 plugin). See `core::oauth2_cache`.
    pub oauth2_cache: Arc<tokio::sync::Mutex<HashMap<String, crate::core::oauth2_cache::CachedToken>>>,
    /// Handle to the kronn-docs Python sidecar (PDF/DOCX/XLSX/… gen).
    /// Always allocated — whether the sidecar is actually running is
    /// stored INSIDE the handle. Routes in `api::docs` probe it at
    /// call time and return a 503 hint if the sidecar was never
    /// brought up (e.g. `make docs-setup` was never run).
    pub docs_sidecar: Arc<crate::core::docs_sidecar::DocsSidecar>,
}

impl AppState {
    /// Canonical factory for an `AppState` — default-initializes every
    /// runtime field (semaphore, trackers, broadcast channel, cancel
    /// registry, OAuth2 cache) from the two caller-supplied inputs.
    ///
    /// **Use this from every main (backend + desktop) instead of
    /// constructing `AppState { ... }` inline.** Struct-literal
    /// construction got us bitten in 0.5.0: when the `oauth2_cache`
    /// field was added to `AppState`, `backend::main` was updated but
    /// `desktop/src-tauri/main` was missed — the backend crate
    /// compiled, `cargo tauri build` broke all 4 desktop OS targets at
    /// release time. A single factory makes drift impossible. Tests
    /// also use this (via `test_state()`) so the happy path is always
    /// exercised.
    pub fn new_defaults(
        config: Arc<RwLock<AppConfig>>,
        db: Arc<Database>,
        max_agents: usize,
    ) -> Self {
        let (ws_tx, _) = tokio::sync::broadcast::channel::<crate::models::WsMessage>(256);
        Self {
            config,
            db,
            agent_semaphore: Arc::new(Semaphore::new(max_agents)),
            audit_tracker: Arc::new(Mutex::new(AuditTracker::default())),
            ws_broadcast: Arc::new(ws_tx),
            cancel_registry: Arc::new(Mutex::new(HashMap::new())),
            oauth2_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            docs_sidecar: Arc::new(crate::core::docs_sidecar::DocsSidecar::new()),
        }
    }
}

/// Helpers to manage the cancel registry without leaking mutex details all
/// over the code. Keep this file-local so the registry convention stays
/// consistent (RAII-style insertion + cleanup on drop).
pub struct CancelGuard {
    registry: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    key: String,
    pub token: tokio_util::sync::CancellationToken,
}

impl CancelGuard {
    /// Register a new cancellation token under `key`, returning a guard that
    /// automatically removes it from the registry when dropped. If a token
    /// already exists under this key (e.g. the user retried a run that never
    /// cleaned up), it's silently replaced — the old token stays dangling
    /// but is no longer reachable, which is fine since CancellationToken is
    /// Arc-based and will be dropped when its last holder goes away.
    pub fn insert(
        registry: &Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
        key: impl Into<String>,
    ) -> Self {
        let key = key.into();
        let token = tokio_util::sync::CancellationToken::new();
        if let Ok(mut map) = registry.lock() {
            map.insert(key.clone(), token.clone());
        }
        Self { registry: registry.clone(), key, token }
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.lock() {
            // Simple remove-by-key: if a later registration replaced this one
            // before Drop runs (rare race), we'd wipe the fresh entry — but
            // that scenario doesn't happen in practice since a disc_id/run_id
            // is only active in one task at a time. If it ever does, the
            // user's ⏹ button would briefly no-op until the next request.
            map.remove(&self.key);
        }
    }
}

// ─── Auth Middleware ─────────────────────────────────────────────────────────

/// Bearer token authentication middleware.
/// - Skips auth for /api/health (Docker healthcheck)
/// - Skips auth when no token is configured
/// - Skips auth for localhost requests (self-hosted: the user is always on the same machine)
/// - Requires Bearer token for remote requests (peers, external API calls)
async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    // Skip auth for health endpoint (Docker healthcheck)
    if request.uri().path() == "/api/health" {
        return Ok(next.run(request).await);
    }

    // Skip auth for WebSocket endpoint — ws.rs handles authentication
    // via invite code verification in the first Presence message.
    if request.uri().path() == "/api/ws" {
        return Ok(next.run(request).await);
    }

    // Skip the bearer check for the cross-instance "claim by token" endpoint —
    // a remote peer has no bearer token. It self-authenticates via its invite
    // code in the body, validated against our contacts in the handler (same
    // trust model as the WS Presence handshake).
    if request.uri().path() == "/api/disc/claim-by-token"
        || request.uri().path() == "/api/disc/fetch-file"
    {
        return Ok(next.run(request).await);
    }

    let config = state.config.read().await;
    let auth_enabled = config.server.auth_enabled;
    let expected_token = config.server.auth_token.clone();
    let strict_localhost = config.server.auth_strict_localhost;
    drop(config);

    // Trust primitives, computed once. `local_trusted` is the self-hosted bypass
    // (a request from a local IP, unless the user opted into strict-localhost);
    // `has_valid_token` is a correct Bearer token. See `is_local_ip` for what
    // counts as local (loopback + Docker bridge gateway, NOT LAN/Tailscale).
    let local_trusted = !strict_localhost && request_is_local_ip(&headers, &request);
    let has_valid_token = match &expected_token {
        Some(expected) => headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t == expected)
            .unwrap_or(false),
        None => false,
    };

    if auth_allows(
        request.method(),
        request.uri().path(),
        auth_enabled,
        expected_token.is_some(),
        local_trusted,
        has_valid_token,
    ) {
        return Ok(next.run(request).await);
    }
    Err(StatusCode::UNAUTHORIZED)
}

/// POST endpoints that irreversibly destroy user data, change key material or
/// mutate the host system. Gated even when app-wide auth is disabled — see
/// `auth_allows`. DELETE routes don't need listing: the VERB itself is the
/// criterion (passe D) — a hand-maintained path list diverged from the route
/// table as soon as new deletes were added.
const DESTRUCTIVE_POSTS: &[&str] = &[
    "/api/setup/reset",
    "/api/config/import",
    "/api/config/recovery/set",
    "/api/config/recovery/restore",
    "/api/audit-runs/cleanup",
    "/api/api-call-logs/purge",
    "/api/debug/logs/clear",
    "/api/agents/uninstall",
    "/api/rtk/deactivate",
];

/// Passe D — the destructive-request criterion: every DELETE (no benign DELETE
/// exists in this API), the listed POSTs, and the parameterized
/// `/api/mcps/custom/{id}/cleanup-orphan-env`.
fn is_destructive(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::DELETE
        || DESTRUCTIVE_POSTS.contains(&path)
        || path.ends_with("/cleanup-orphan-env")
}

/// Pure auth decision for a request that already cleared the always-open
/// exceptions (health, ws, claim-by-token). `true` = allow.
///
/// The one non-obvious rule (I9): DESTRUCTIVE endpoints require local trust or a
/// valid token EVEN when `auth_enabled` is false. The Docker default is auth-off
/// on 0.0.0.0, so a bare `!auth_enabled` pass would let any LAN/Tailscale peer
/// wipe the instance. Everything else keeps the historical behaviour: auth off →
/// open; no token configured → open; otherwise local-bypass-or-token.
fn auth_allows(
    method: &axum::http::Method,
    path: &str,
    auth_enabled: bool,
    token_configured: bool,
    local_trusted: bool,
    has_valid_token: bool,
) -> bool {
    if is_destructive(method, path) {
        return local_trusted || has_valid_token;
    }
    if !auth_enabled {
        return true;
    }
    if !token_configured {
        return true;
    }
    local_trusted || has_valid_token
}

/// True when the request originates from a local IP — either the `X-Real-IP`
/// nginx sets (Docker), or the direct peer address (Tauri desktop / no proxy).
/// Does NOT apply the strict-localhost policy; callers combine it as needed.
fn request_is_local_ip(headers: &HeaderMap, request: &axum::extract::Request) -> bool {
    // Passe D — `X-Real-IP` is only trustworthy when the bundled nginx sets
    // it (Docker). On a native bind axum talks to clients DIRECTLY, so the
    // header is attacker-controlled: a LAN peer sending `X-Real-IP:
    // 127.0.0.1` used to mint local trust and bypass the destructive gate.
    if crate::core::env::is_docker() {
        if let Some(real_ip) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            if is_local_ip(real_ip) {
                return true;
            }
        }
    }
    if let Some(ci) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        if is_local_ip(&ci.0.ip().to_string()) {
            return true;
        }
    }
    false
}

/// Check if an IP address is local (localhost or Docker internal network).
/// Docker bridge uses 172.16-31.x.x; the host accesses the gateway from
/// an IP like 172.18.0.1. Peers connect from Tailscale (100.x), LAN (192.168.x),
/// or public IPs — those are NOT local.
///
/// Strings only — HTTP source IPs always arrive as numeric strings
/// (axum's `SocketAddr::ip()` already resolves hostnames upstream).
/// We previously matched "localhost" defensively, but that opened a
/// tiny attack surface where a misconfigured nginx could forge
/// `X-Real-IP: localhost` to bypass auth. Numeric-only is safer.
fn is_local_ip(ip: &str) -> bool {
    if ip == "127.0.0.1" || ip == "::1" {
        return true;
    }
    // Docker bridge: 172.16.0.0/12
    if let Some(rest) = ip.strip_prefix("172.") {
        if let Some(second) = rest.split('.').next().and_then(|s| s.parse::<u8>().ok()) {
            return (16..=31).contains(&second);
        }
    }
    false
}

// ─── CORS ────────────────────────────────────────────────────────────────────

/// Build CORS layer based on config domain.
fn build_cors(domain: &Option<String>, port: u16) -> CorsLayer {
    let origins: Vec<String> = match domain {
        Some(d) => vec![
            format!("https://{}", d),
            format!("http://{}", d),
            format!("https://{}:{}", d, port),
            format!("http://{}:{}", d, port),
        ],
        None => vec![
            format!("http://localhost:{}", port),
            format!("http://127.0.0.1:{}", port),
            // Default gateway port
            "http://localhost:3140".into(),
            "http://localhost:3141".into(),
        ],
    };

    let parsed: Vec<_> = origins
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(parsed))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Build the Axum router with all routes and middleware.
/// Extracted for reuse in integration tests.
pub fn build_router(state: AppState) -> Router {
    build_router_with_auth(state, true)
}

/// Build router with optional auth (disabled for tests).
pub fn build_router_with_auth(state: AppState, enable_auth: bool) -> Router {
    let (domain, port) = {
        // We need config synchronously here; use blocking_lock since this runs once at startup
        let config = state.config.try_read().expect("Config lock poisoned at startup — cannot build router. Check for panic in config initialization.");
        (config.server.domain.clone(), config.server.port)
    };

    let mut router = Router::new()
        // ── Health (lightweight, used by Docker healthcheck — no auth) ──
        // `version` + `host_os` are included so the Settings > Debug "Report
        // a bug on GitHub" button can stamp them into the issue template
        // without hitting an authenticated endpoint first. Docker's curl-based
        // healthcheck ignores the body, so adding fields is backwards-safe.
        .route("/api/health", get(|| async {
            axum::Json(serde_json::json!({
                "ok": true,
                "version": env!("CARGO_PKG_VERSION"),
                "host_os": crate::agents::detect_host_label_public(),
                // Lets the UI gate the "Install agent" button: under Docker the
                // backend runs in a Linux container that can't install onto the
                // host, so the UI points to the host-side `kronn` CLI instead.
                // Native (Tauri/CLI) → false → Install works on the host.
                "in_docker": crate::core::env::is_docker(),
            }))
        }))
        // ── Setup wizard ──
        .route("/api/open-url", post(api::setup::open_url))
        .route("/api/setup/status", get(api::setup::get_status))
        .route("/api/setup/scan-paths", post(api::setup::set_scan_paths))
        .route("/api/setup/install-agent", post(api::setup::install_agent))
        .route("/api/setup/complete", post(api::setup::complete))
        .route("/api/setup/reset", post(api::setup::reset))
        // ── Version check (auto-update banner) ──
        .route("/api/version/check", get(api::version::check))
        // ── Agent CLI usage / cost (via ccusage) ──
        .route("/api/usage", get(api::usage::get_usage))
        // ── OpenAPI / Swagger UI ──
        // Spec served at `/api/openapi.json` by SwaggerUi (its `.url()`
        // mounts the spec route automatically). Interactive UI at
        // `/api/docs`. Hand-curated; new endpoints opt in via the
        // `#[utoipa::path]` macro and a `paths(...)` entry in
        // `api::openapi::ApiDoc`.
        .merge(utoipa_swagger_ui::SwaggerUi::new("/api/docs").url("/api/openapi.json", api::openapi::openapi_spec()))
        // ── Config ──
        .route("/api/config/tokens", get(api::setup::get_tokens))
        .route("/api/config/api-keys", post(api::setup::save_api_key))
        .route("/api/config/api-keys/{id}", delete(api::setup::delete_api_key))
        .route("/api/config/api-keys/{id}/activate", post(api::setup::activate_api_key))
        .route("/api/config/sync-agent-tokens", post(api::setup::sync_agent_tokens))
        .route("/api/config/discover-keys", post(api::setup::discover_keys))
        .route("/api/config/toggle-token-override", post(api::setup::toggle_token_override))
        .route("/api/config/language", get(api::setup::get_language).post(api::setup::save_language))
        .route("/api/config/ui-language", get(api::setup::get_ui_language).post(api::setup::save_ui_language))
        .route("/api/config/stt-model", get(api::setup::get_stt_model).post(api::setup::save_stt_model))
        .route("/api/config/tts-voices", get(api::setup::get_tts_voices))
        .route("/api/config/tts-voice", post(api::setup::save_tts_voice))
        .route("/api/config/global-context", get(api::setup::get_global_context).post(api::setup::save_global_context))
        .route("/api/config/global-context-mode", get(api::setup::get_global_context_mode).post(api::setup::save_global_context_mode))
        // 0.8.7 anti-hallucination mode (off | warn | enforce).
        .route("/api/config/anti-hallucination-mode", get(api::setup::get_anti_hallucination_mode).post(api::setup::save_anti_hallucination_mode))
        .route("/api/config/continual-learning-enabled", get(api::setup::get_continual_learning_enabled).post(api::setup::save_continual_learning_enabled))
        // 0.8.7 — spec doc served from include_str! (linked from Settings → Sourcing).
        .route("/api/conventions/agents-md-format-v1", get(api::setup::get_agents_md_spec_v1))
        // P2 recovery passphrase (Argon2id-wrapped key). set/restore are
        // auth-gated as destructive-adjacent — see DESTRUCTIVE_PATHS.
        .route("/api/config/recovery/status", get(api::setup::recovery_status))
        .route("/api/config/recovery/set", post(api::setup::set_recovery))
        .route("/api/config/recovery/restore", post(api::setup::restore_recovery))
        .route("/api/config/scan-paths", get(api::setup::get_scan_paths).post(api::setup::set_scan_paths))
        .route("/api/config/scan-ignore", get(api::setup::get_scan_ignore).post(api::setup::set_scan_ignore))
        .route("/api/config/scan-depth", get(api::setup::get_scan_depth).post(api::setup::set_scan_depth))
        .route("/api/config/agent-access", get(api::setup::get_agent_access).post(api::setup::set_agent_access))
        .route("/api/config/model-tiers", get(api::setup::get_model_tiers).post(api::setup::set_model_tiers))
        .route("/api/config/server", get(api::setup::get_server_config).post(api::setup::set_server_config))
        .route("/api/config/network-exposure", get(api::setup::get_network_exposure).post(api::setup::set_network_exposure))
        .route("/api/config/auth-token/regenerate", post(api::setup::regenerate_auth_token))
        .route("/api/config/db-info", get(api::setup::db_info))
        .route("/api/db/backup", post(api::setup::db_backup))
        .route("/api/config/export", get(api::setup::export_data))
        // Whole-DB restore: exports routinely exceed axum's ~2 MB default body
        // limit (a few hundred discussions ≈ 2 MB ZIP). Without this, any
        // non-trivial export fails the upload with "Error parsing
        // multipart/form-data request" before the data is ever read. 512 MB
        // headroom — this is a trusted localhost admin op.
        .route(
            "/api/config/import",
            post(api::setup::import_data)
                .layer(axum::extract::DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        // ── Projects ──
        .route("/api/projects", get(api::projects::list))
        .route("/api/projects", post(api::projects::create))
        .route("/api/projects/scan", post(api::projects::scan))
        .route("/api/projects/add-folder", post(api::projects::add_folder))
        .route("/api/projects/bootstrap", post(api::projects::bootstrap))
        .route("/api/projects/clone", post(api::projects::clone_project))
        .route("/api/projects/discover-repos", post(api::discover::discover_repos))
        .route("/api/projects/{id}", get(api::projects::get))
        .route("/api/projects/{id}", delete(api::projects::delete))
        .route("/api/projects/{id}/install-template", post(api::projects::install_template))
        // 0.8.7 anti-hallu migration : inject the canonical section into
        // pre-existing projects + re-sync redirectors. Both idempotent.
        .route("/api/projects/{id}/anti-hallu/status", get(api::projects::anti_hallu_inject::status))
        .route("/api/projects/{id}/anti-hallu/inject", post(api::projects::anti_hallu_inject::inject))
        .route("/api/projects/{id}/redirectors/sync", post(api::projects::anti_hallu_inject::sync_redirectors))
        .route("/api/projects/{id}/audit-info", get(api::audit::audit_info))
        .route("/api/projects/{id}/drift", get(api::audit::check_drift))
        .route("/api/projects/{id}/partial-audit", post(api::audit::partial_audit))
        .route("/api/projects/{id}/validate-audit", post(api::audit::validate_audit))
        .route("/api/projects/{id}/mark-bootstrapped", post(api::audit::mark_bootstrapped))
        .route("/api/projects/{id}/full-audit", post(api::audit::full_audit))
        .route("/api/projects/{id}/cancel-audit", post(api::audit::cancel_audit))
        .route("/api/projects/{id}/audit-status", get(api::audit::audit_status))
        // 0.8.3 (#288) — fleet-wide view of every running audit.
        .route("/api/audit-status", get(api::audit::audit_status_all))
        // 0.8.3 (#311) — last resumable audit run for a project. Drives
        // the "Reprendre Step N/10" button on the ProjectCard when an
        // earlier run was interrupted (rate-limit, crash, network blip).
        .route("/api/projects/{id}/audit-resumable", get(api::audit::audit_latest_resumable))
        // 0.8.4 (#298) — last completed audit + per-step metrics for the
        // ProjectCard recap panel. Read-only.
        .route("/api/projects/{id}/audit-latest", get(api::audit::audit_latest))
        .route("/api/projects/{id}/audit-history", get(api::audit::audit_history))
        .route("/api/audit-runs/{run_id}/steps", get(api::audit::audit_run_steps))
        // 0.8.4 (#317 / B1) — admin: force-clear all `Running` audit_runs.
        // The boot hook handles the 30-min threshold automatically; this
        // is the manual escape hatch for operators.
        .route("/api/audit-runs/cleanup", post(api::audit::audit_runs_cleanup))
        .route("/api/projects/{id}/remap-path", post(api::projects::remap_path))
        // Recover a project whose path no longer resolves (cross-machine
        // import): re-clone its repo_url locally + re-point the existing
        // project at the clone. Re-syncs plugins/skills to the new path.
        .route("/api/projects/{id}/clone-and-remap", post(api::projects::clone_and_remap))
        // 0.7.1 — `ai/` → `docs/` convention migration. Idempotent, safe
        // to call on already-migrated or never-bootstrapped projects.
        .route("/api/projects/{id}/migrate-docs", post(api::projects::migrate_docs))
        // 0.7.1 — User-scoped context CRUD : files in ~/.kronn/user-context/
        // are auto-injected into every agent's prompt. UI-editable so the
        // operator never needs a terminal.
        .route("/api/user-context", get(api::user_context::list))
        .route("/api/user-context/{name}",
            get(api::user_context::get)
                .put(api::user_context::put)
                .delete(api::user_context::delete))
        .route("/api/projects/{id}/default-skills", put(api::projects::set_default_skills))
        .route("/api/projects/{id}/default-profile", put(api::projects::set_default_profile))
        // 0.8.3 — companion repos. Body = full Vec<LinkedRepo>;
        // atomic replace (no partial CRUD per row).
        .route("/api/projects/{id}/linked-repos", put(api::projects::set_linked_repos))
        // 0.8.6 (#27) — autocomplete picker source for the
        // linked-repos drawer. Returns other Kronn-known projects
        // sorted by proximity.
        .route("/api/projects/{id}/linked-repos/candidates", get(api::projects::linked_repos_candidates))
        .route("/api/projects/{id}/briefing", get(api::audit::get_briefing).put(api::audit::set_briefing))
        .route("/api/projects/{id}/start-briefing", post(api::audit::start_briefing))
        // 0.8.4 (#285) — désagentified briefing form. POST the 6 answers
        // directly, server writes docs/briefing.md + persists DB notes,
        // no LLM call. Coexists with the conversational variant above.
        .route("/api/projects/{id}/save-briefing", post(api::audit::save_briefing_form))
        .route("/api/projects/{id}/ai-files", get(api::ai_docs::list_ai_files))
        .route("/api/projects/{id}/ai-file", get(api::ai_docs::read_ai_file))
        .route("/api/projects/{id}/ai-search", get(api::ai_docs::search_ai_files))
        .route("/api/projects/{id}/doc-asset", get(api::ai_docs::read_doc_asset))
        .route("/api/projects/{id}/git-status", get(api::projects::git_status))
        .route("/api/projects/{id}/git-diff", get(api::projects::git_diff))
        .route("/api/projects/{id}/git-branch", post(api::projects::git_branch))
        .route("/api/projects/{id}/git-commit", post(api::projects::git_commit))
        .route("/api/projects/{id}/git-push", post(api::projects::git_push))
        .route("/api/projects/{id}/git-pr", post(api::projects::create_pr))
        .route("/api/projects/{id}/pr-template", get(api::projects::pr_template))
        .route("/api/projects/{id}/exec", post(api::projects::project_exec))
        .route("/api/projects/{id}/workflow-suggestions", get(api::workflows::suggestions))
        // ── Agents ──
        .route("/api/agents", get(api::agents::detect))
        .route("/api/agents/install", post(api::agents::install))
        .route("/api/agents/uninstall", post(api::agents::uninstall))
        .route("/api/agents/toggle", post(api::agents::toggle))
        // ── RTK (Rust Token Killer) — host-side compression proxy ──
        .route("/api/rtk/activate", post(api::rtk::activate))
        .route("/api/rtk/deactivate", post(api::rtk::deactivate))
        .route("/api/rtk/savings", get(api::rtk::savings))
        .route("/api/rtk/version", get(api::rtk::version))
        // ── Ollama (local LLM) ──
        .route("/api/ollama/health", get(api::ollama::health))
        .route("/api/ollama/models", get(api::ollama::models))
        // ── Debug (log ringbuffer — backs Settings > Debug viewer) ──
        .route("/api/debug/logs", get(api::debug::get_logs))
        .route("/api/debug/logs/clear", post(api::debug::clear_logs))
        // ── Secret themes (hidden palette unlock via code) ──
        .route("/api/themes/unlock", post(api::themes::unlock))
        // ── Document generation (5 formats through the Python sidecar) ──
        .route("/api/docs/pdf", post(api::docs::generate_pdf))
        .route("/api/docs/docx", post(api::docs::generate_docx))
        .route("/api/docs/xlsx", post(api::docs::generate_xlsx))
        .route("/api/docs/csv", post(api::docs::generate_csv))
        .route("/api/docs/pptx", post(api::docs::generate_pptx))
        .route("/api/docs/file/{discussion_id}/{filename}", get(api::docs::download_file))
        // ── MCPs ──
        .route("/api/mcps", get(api::mcps::overview))
        .route("/api/mcps/registry", get(api::mcps::list_registry))
        .route("/api/mcps/refresh", post(api::mcps::refresh))
        .route("/api/mcps/configs", post(api::mcps::create_config))
        .route("/api/mcps/configs/{id}", patch(api::mcps::update_config).delete(api::mcps::delete_config))
        // 0.8.6 — Custom API plugin spec edit. Lets the user fix a
        // typo / add endpoints / change docs_url WITHOUT delete+recreate.
        // Server_id is preserved; configs & workflow ApiCall refs stay valid.
        .route("/api/mcps/custom/{server_id}", put(api::mcps::update_custom_spec))
        // 0.8.6 (#60) — cleanup orphan env keys left behind by a field
        // rename / removal. Body: { keys: ["OLD_KEY", …] }.
        .route(
            "/api/mcps/custom/{server_id}/cleanup-orphan-env",
            post(api::mcps::cleanup_orphan_env),
        )
        // 0.8.6 (#63) — Path B file-based plugin import/export.
        // GET returns a .kronn-plugin.json attachment ; POST accepts the
        // same JSON shape (frontend reads File → text → POST).
        .route(
            "/api/mcps/custom/{server_id}/export-file",
            get(api::mcps::export_custom_plugin_file),
        )
        .route(
            "/api/mcps/custom/import-file",
            post(api::mcps::import_custom_plugin_file),
        )
        .route("/api/mcps/configs/{id}/projects", patch(api::mcps::set_config_projects))
        .route("/api/mcps/configs/{id}/reveal", post(api::mcps::reveal_secrets))
        .route("/api/mcps/host-discovery", get(api::mcps::host_discovery))
        .route("/api/mcps/host-discovery/adopt", post(api::mcps::adopt_host_mcp))
        .route("/api/mcps/context/{project_id}", get(api::mcps::list_contexts))
        .route("/api/mcps/context/{project_id}/{slug}", get(api::mcps::get_context).put(api::mcps::update_context))
        // ── Workflows ──
        .route("/api/workflows", get(api::workflows::list).post(api::workflows::create))
        // 0.8.3 — Feasibility-Gated Implementation: one-shot template
        // creation for big tickets. POST `{project_id, ticket_ref?,
        // ticket_body?, agent?, name?}` → returns a 5-step workflow.
        .route("/api/workflows/templates/feasibility-autopilot", post(api::workflows::create_feasibility_autopilot))
        // 0.8.3 — Bundle creator. Atomic creation of (Quick Prompts
        // × N) + (Quick APIs × N) + (Custom API plugins × N) +
        // (1 Workflow) from a single `KRONN:BUNDLE_READY` chat
        // signal. Single SQLite transaction — rollback on any error.
        // See `api::bundle` for the wire shape + ref-resolution
        // protocol (`@bundle:<id>` placeholders).
        .route("/api/workflows/bundle", post(api::bundle::create_bundle))
        // 0.8.3 — Feasibility-Gated traceability surface. Read-only
        // for now; mutation (override / mark resolved) lands once the
        // frontend Decision-log page does.
        .route("/api/agent-decisions", get(api::workflows::list_agent_decisions))
        .route("/api/workflows/{id}", get(api::workflows::get).put(api::workflows::update).delete(api::workflows::delete))
        .route("/api/workflows/test-step", post(api::workflows::test_step))
        .route("/api/workflows/test-batch-step", post(api::workflows::test_batch_step))
        // ── ApiCall step wizard endpoints (P0.5 — désagentification) ──
        .route("/api/workflow-steps/test-extract", post(api::workflows::test_extract))
        .route("/api/workflow-steps/test-api-call", post(api::workflows::test_api_call))
        // 0.8.6 — Agent API broker. Lets the kronn-internal MCP forward
        // an agent-driven HTTP call through the same executor as
        // workflow ApiCall steps. Credentials never leave Kronn DB.
        // Project scope resolved from the parent disc.
        .route("/api/agent-api/call", post(api::agent_api::agent_api_call))
        // 0.8.6 phase 4 — MCP remote control (workflow_trigger / workflow_run_status / qp_run).
        // JSON wrappers around the SSE-based trigger/run/batch routes,
        // enriched with smart-polling `next_check` hints so an MCP-driven
        // agent on mobile can launch + track without burning tokens on
        // SSE chunks it can't easily consume.
        .route("/api/mcp/workflow-trigger", post(api::mcp_remote::workflow_trigger))
        .route("/api/mcp/workflow-run-status/{run_id}", get(api::mcp_remote::workflow_run_status))
        .route("/api/mcp/qp-run", post(api::mcp_remote::qp_run))
        // 0.8.7 phase 4 — PR2 (batch fan-out + run discussions) + PR3 (long-poll wait).
        .route("/api/mcp/qp-batch-run", post(api::mcp_remote::qp_batch_run))
        .route("/api/mcp/workflow-run-discussions/{run_id}", get(api::mcp_remote::workflow_run_discussions))
        .route("/api/mcp/workflow-wait-for-completion", post(api::mcp_remote::workflow_wait_for_completion))
        // 0.8.6 (#24) — unified API-call logs read surface. Lists / shows
        // / purges rows from `api_call_logs` (workflow + broker + manual).
        .route("/api/api-call-logs", get(api::api_call_logs::list_api_call_logs))
        .route("/api/api-call-logs/purge", post(api::api_call_logs::purge_api_call_logs))
        .route("/api/api-call-logs/{id}", get(api::api_call_logs::get_api_call_log))
        // 0.9.0 — Continual Learning (spec docs/research/continual-learning-0.9.0-spec.md)
        .route("/api/learnings/propose", post(api::learnings::propose_learning))
        .route("/api/learnings", get(api::learnings::list_learnings))
        .route("/api/learnings/pending", get(api::learnings::pending_count))
        .route("/api/learnings/{id}/validate", post(api::learnings::validate_learning))
        .route("/api/learnings/{id}/reject", post(api::learnings::reject_learning))
        .route("/api/discussions/{id}/learnings", get(api::learnings::disc_learnings))
        .route("/api/projects/{id}/learnings/sync", post(api::learnings::sync_learnings_doc))
        .route("/api/workflows/{id}/trigger", post(api::workflows::trigger))
        .route("/api/workflows/{id}/runs", get(api::workflows::list_runs).delete(api::workflows::delete_all_runs))
        .route("/api/workflows/{id}/runs/{run_id}", get(api::workflows::get_run).delete(api::workflows::delete_run))
        .route("/api/workflows/{id}/runs/{run_id}/cancel", post(api::workflows::cancel_run))
        .route("/api/workflows/{id}/runs/{run_id}/decide", post(api::workflows::decide_run))
        .route(
            "/api/workflows/{id}/runs/{run_id}/test-worktree",
            post(api::workflows::test_worktree).delete(api::workflows::delete_test_worktree),
        )
        // 0.7.0 UX pass — per-item export / import (single workflow or QP).
        // Distinct from /api/config/export which exports the whole DB.
        .route("/api/workflows/{id}/export", get(api::workflows::export_workflow))
        .route("/api/workflows/import", post(api::workflows::import_workflow))
        .route("/api/workflow-runs/batch-summaries", get(api::workflows::list_batch_run_summaries))
        .route("/api/workflow-runs/{run_id}", delete(api::workflows::delete_batch_run))
        .route("/api/workflow-runs/{run_id}/resume", post(api::workflows::resume_interrupted))
        // ── Quick Prompts ──
        .route("/api/quick-prompts", get(api::quick_prompts::list).post(api::quick_prompts::create))
        .route("/api/quick-prompts/{id}", put(api::quick_prompts::update).delete(api::quick_prompts::delete))
        .route("/api/quick-prompts/{id}/batch", post(api::quick_prompts::batch_run))
        // Compare-agents mode — fan out the same prompt across N agents.
        .route("/api/quick-prompts/{id}/compare-agents", post(api::quick_prompts::compare_agents))
        // 0.8.5 — version history + per-version metrics for the QP
        // history drawer (avg tokens, avg duration, avg cost per
        // version_index).
        .route("/api/quick-prompts/{id}/history", get(api::quick_prompts::history))
        .route("/api/quick-prompts/{id}/metrics", get(api::quick_prompts::metrics))
        // 0.8.5 — drop an archived QP version. Refused on the current
        // (highest) version_index; cascades originating_qp_* on discs
        // referencing the deleted version to NULL.
        .route("/api/quick-prompts/{id}/versions/{version_index}", delete(api::quick_prompts::delete_version))
        // 0.7.0 UX pass — per-item export / import.
        .route("/api/quick-prompts/{id}/export", get(api::quick_prompts::export_qp))
        .route("/api/quick-prompts/import", post(api::quick_prompts::import_qp))
        // ── Quick APIs (0.6.0 — reusable HTTP call templates) ──
        .route("/api/quick-apis", get(api::quick_apis::list).post(api::quick_apis::create))
        .route("/api/quick-apis/{id}", put(api::quick_apis::update).delete(api::quick_apis::delete))
        .route("/api/quick-apis/{id}/run", post(api::quick_apis::run_qa))
        .route("/api/quick-apis/{id}/batch", post(api::quick_apis::batch_run_qa))
        .route("/api/quick-apis/{id}/export", get(api::quick_apis::export_qa))
        .route("/api/quick-apis/import", post(api::quick_apis::import_qa))
        // ── Discussions ──
        .route("/api/discussions", get(api::discussions::list))
        .route("/api/discussions", post(api::discussions::create))
        // Static segment BEFORE the `{id}` capture so it isn't swallowed by it.
        .route("/api/discussions/running", get(api::discussions::running_discussions))
        .route("/api/discussions/{id}", get(api::discussions::get))
        .route("/api/discussions/{id}", delete(api::discussions::delete).patch(api::discussions::update))
        .route("/api/discussions/{id}/messages", post(api::discussions::send_message))
        .route("/api/discussions/{id}/messages/last", delete(api::discussions::delete_last_agent_messages).patch(api::discussions::edit_last_user_message))
        .route("/api/discussions/{id}/run", post(api::discussions::run_agent))
        .route("/api/discussions/{id}/stop", post(api::discussions::stop_agent))
        .route("/api/discussions/{id}/dismiss-partial", post(api::discussions::dismiss_partial))
        .route("/api/discussions/{id}/orchestrate", post(api::discussions::orchestrate))
        .route("/api/discussions/{id}/share", post(api::discussions::share))
        // 0.8.6 phase 2 — cross-agent collab : invite a peer agent.
        .route("/api/discussions/{id}/invite-peer", post(api::disc_invite::invite_peer))
        // List the active participants of a disc — header rendering.
        .route("/api/discussions/{id}/participants", get(api::disc_invite::list_participants))
        // 0.8.6 phase 3 — long-poll for new peer messages.
        .route("/api/discussions/{id}/wait", get(api::disc_invite::wait_for_peer))
        // Companion : the bridge calls this from `disc_join({token})`
        // to validate the token and bind itself to the resolved disc.
        // Not scoped by id — the disc identity is what the token resolves to.
        .route("/api/discussions/peer-join", post(api::disc_invite::peer_join))
        .route("/api/discussions/peer-resume", post(api::disc_invite::peer_resume))
        // Cross-instance leg of the unified "join by code": a peer asks whether
        // we host the room behind a token; if so we share it back. Auth-exempt
        // (self-auth via invite code in body — see auth_middleware).
        .route("/api/disc/claim-by-token", post(api::disc_invite::claim_by_token))
        .route("/api/disc/fetch-file", post(api::disc_invite::fetch_file))
        // 0.8.6 phase 3 — `disc_leave` MCP tool's companion route.
        // Marks the caller's active session as `left` (idempotent).
        .route("/api/discussions/peer-leave", post(api::disc_invite::peer_leave))
        // Introspection endpoints — surface the conversation as a queryable
        // resource for the agent (see api::disc_introspection). The
        // `kronn-internal` MCP bridge calls these via HTTP from the
        // agent's process, letting the agent decide at runtime whether
        // it needs metadata, a specific message, or an on-demand summary.
        .route("/api/discussions/{id}/meta", get(api::disc_introspection::disc_meta))
        .route("/api/discussions/{id}/message/{idx}", get(api::disc_introspection::disc_get_message))
        .route("/api/discussions/{id}/summarize", post(api::disc_introspection::disc_summarize))
        // 0.8.4 (#294) — cross-agent memory routes. Each one is a
        // 1:1 mirror of an MCP tool exposed by `disc-introspection-mcp.py`,
        // so a Claude Code (or any compatible) session can push its
        // history into Kronn DB and let a different agent pick it up
        // later. See `project_cross_agent_memory_0_8_4.md`.
        .route("/api/disc/create",            post(api::disc_source::disc_create))
        .route("/api/disc/append",            post(api::disc_source::disc_append))
        .route("/api/disc/link",              post(api::disc_source::disc_link))
        .route("/api/disc/unlink",            post(api::disc_source::disc_unlink))
        .route("/api/disc/find_by_session",   get(api::disc_source::disc_find_by_session))
        .route("/api/disc/search",            get(api::disc_source::disc_search))
        .route("/api/disc/load_other",        get(api::disc_source::disc_load_other))
        // 0.8.4 (#294) UI-facing readers — let the frontend decorate
        // the sidebar with "imported from X" badges + drive the
        // source-filter dropdown.
        .route("/api/disc/sources",           get(api::disc_source::list_source_bindings))
        .route("/api/discussions/{id}/source", get(api::disc_source::disc_source_detail))
        .route("/api/discussions/{id}/git-status", get(api::disc_git::disc_git_status))
        .route("/api/discussions/{id}/git-diff", get(api::disc_git::disc_git_diff))
        .route("/api/discussions/{id}/git-commit", post(api::disc_git::disc_git_commit))
        .route("/api/discussions/{id}/git-push", post(api::disc_git::disc_git_push))
        .route("/api/discussions/{id}/git-pr", post(api::disc_git::disc_create_pr))
        .route("/api/discussions/{id}/pr-template", get(api::disc_git::disc_pr_template))
        .route("/api/discussions/{id}/exec", post(api::disc_git::disc_exec))
        .route("/api/discussions/{id}/worktree-unlock", post(api::disc_git::worktree_unlock))
        .route("/api/discussions/{id}/worktree-lock", post(api::disc_git::worktree_lock))
        .route("/api/discussions/{id}/test-mode/enter", post(api::disc_git::test_mode_enter))
        .route("/api/discussions/{id}/test-mode/exit", post(api::disc_git::test_mode_exit))
        // ── Context Files ──
        // Upload accepts files up to 64MB (axum's default body limit is ~2MB —
        // far too small for a HAR / log / dataset attached to be read off disk).
        // The handler still routes images (≤10MB) + office docs to their own
        // caps; everything else lands on disk with only a preview inlined.
        .route(
            "/api/discussions/{id}/context-files",
            get(api::discussions::list_context_files)
                .post(api::discussions::upload_context_file)
                .layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
        .route("/api/discussions/{id}/context-files/{file_id}", delete(api::discussions::delete_context_file))
        .route("/api/discussions/{id}/context-files/{file_id}/content", get(api::discussions::get_context_file_content))
        .route("/api/discussions/{id}/context-files/link-pending", post(api::discussions::link_pending_context_files))
        // ── WebSocket ──
        .route("/api/ws", get(api::ws::ws_handler))
        // ── Contacts ──
        .route("/api/contacts", get(api::contacts::list).post(api::contacts::add))
        .route("/api/contacts/invite-code", get(api::contacts::invite_code))
        .route("/api/contacts/network-info", get(api::contacts::network_info))
        .route("/api/contacts/{id}", delete(api::contacts::delete))
        .route("/api/contacts/{id}/ping", get(api::contacts::ping))
        // ── Skills ──
        .route("/api/skills", get(api::skills::list).post(api::skills::create))
        .route("/api/skills/{id}", put(api::skills::update).delete(api::skills::delete))
        .route("/api/skills/auto-triggers/disabled", get(api::skills::list_disabled_auto))
        .route("/api/skills/{id}/auto-trigger/toggle", post(api::skills::toggle_auto_trigger))
        // ── Profiles ──
        .route("/api/profiles", get(api::profiles::list).post(api::profiles::create))
        .route("/api/profiles/{id}", get(api::profiles::get).put(api::profiles::update).delete(api::profiles::delete))
        .route("/api/profiles/{id}/persona-name", put(api::profiles::update_persona_name))
        // ── Directives ──
        .route("/api/directives", get(api::directives::list).post(api::directives::create))
        .route("/api/directives/{id}", put(api::directives::update).delete(api::directives::delete))
        // ── Stats ──
        .route("/api/stats/tokens", get(api::stats::token_usage))
        .route("/api/stats/agent-usage", get(api::stats::agent_usage))
        // ── Middleware ──
        .layer(build_cors(&domain, port))
        .layer(TraceLayer::new_for_http());

    if enable_auth {
        router = router.route_layer(middleware::from_fn_with_state(state.clone(), auth_middleware));
    }

    router.with_state(state)
}

#[cfg(test)]
mod audit_tracker_tests {
    use super::AuditTracker;

    #[test]
    fn start_progress_seeds_an_entry_and_overwrites_stale_rows() {
        let mut t = AuditTracker::default();
        // Seed an explicitly stale entry to prove start_progress resets it.
        t.start_progress("proj-a", 3, "full");
        t.advance_step("proj-a", 2, Some("old.md".into()));
        t.start_progress("proj-a", 10, "full_audit");

        let p = t.get_progress("proj-a").unwrap();
        assert_eq!(p.total_steps, 10);
        assert_eq!(p.step_index, 0, "start_progress must reset step_index");
        assert_eq!(p.current_file, None, "start_progress must clear current_file");
        assert_eq!(p.kind, "full_audit");
        assert_eq!(p.phase, "auditing");
    }

    #[test]
    fn advance_step_updates_counter_and_file_label() {
        let mut t = AuditTracker::default();
        t.start_progress("proj-b", 10, "full");
        t.advance_step("proj-b", 3, Some("repo-map.md".into()));

        let p = t.get_progress("proj-b").unwrap();
        assert_eq!(p.step_index, 3);
        assert_eq!(p.current_file.as_deref(), Some("repo-map.md"));
    }

    #[test]
    fn advance_step_is_noop_when_no_progress_exists() {
        // advance_step must not silently create a progress entry — callers
        // would otherwise see "running" for audits that never started.
        let mut t = AuditTracker::default();
        t.advance_step("ghost", 1, Some("x.md".into()));
        assert!(t.get_progress("ghost").is_none());
    }

    // ─── 0.8.4 (#319 / B3) tool-call counter ────────────────────────

    #[test]
    fn tool_call_count_increments_on_each_tool_update() {
        // The user-facing chip reads `🔧 Tool (N)` — N must reflect
        // how many tool_call events have hit since step_start so the
        // user sees forward motion even when the agent is in a long
        // tool-only phase (Read/Bash/Write loop) without a `Usage`
        // block to refresh the token chip. Counter resets per-step.
        let mut t = AuditTracker::default();
        t.start_progress("p-tc", 10, "full");
        // Step 1 — three tool calls.
        t.advance_step("p-tc", 1, Some("AGENTS.md".into()));
        t.update_chips("p-tc", None, None, Some("Read".into()));
        t.update_chips("p-tc", None, None, Some("Bash".into()));
        t.update_chips("p-tc", None, None, Some("Write".into()));
        {
            let p = t.get_progress("p-tc").unwrap();
            assert_eq!(p.current_tool.as_deref(), Some("Write"), "last-tool wins");
            assert_eq!(p.current_tool_call_count, Some(3), "counter bumps on every tool update");
        }

        // Step 2 — counter resets via `clear_step_chips` (fired by
        // the SSE pipeline at step_start).
        t.clear_step_chips("p-tc");
        t.advance_step("p-tc", 2, Some("repo-map.md".into()));
        {
            let p = t.get_progress("p-tc").unwrap();
            assert_eq!(p.current_tool_call_count, None, "counter resets at step boundary");
        }
        t.update_chips("p-tc", None, None, Some("Grep".into()));
        {
            let p = t.get_progress("p-tc").unwrap();
            assert_eq!(p.current_tool_call_count, Some(1), "first tool in new step → counter = 1");
        }
    }

    #[test]
    fn token_chip_updates_do_not_bump_tool_counter() {
        // `update_chips` is also called with token updates (no tool).
        // Those must NOT increment the tool counter — otherwise a
        // burst of `Usage` events without a real tool change would
        // produce a misleading "N tool calls" chip.
        let mut t = AuditTracker::default();
        t.start_progress("p-tok", 10, "full");
        t.advance_step("p-tok", 1, Some("AGENTS.md".into()));
        t.update_chips("p-tok", Some(100), Some(100), None);
        t.update_chips("p-tok", Some(250), Some(250), None);
        t.update_chips("p-tok", Some(400), Some(400), None);
        let p = t.get_progress("p-tok").unwrap();
        assert_eq!(p.step_tokens, Some(400));
        assert_eq!(p.current_tool_call_count, None, "token-only updates must NOT bump the counter");
    }

    #[test]
    fn clear_progress_removes_the_entry() {
        let mut t = AuditTracker::default();
        t.start_progress("proj-c", 5, "partial");
        t.clear_progress("proj-c");
        assert!(t.get_progress("proj-c").is_none());
    }

    #[test]
    fn audit_lease_is_exclusive_per_project() {
        // The single atomic gate for Full/specialized/partial: the first
        // acquire wins, a second is refused until release. Different projects
        // are independent.
        let mut t = AuditTracker::default();
        assert!(t.try_acquire_lease("p1"), "first acquire wins");
        assert!(!t.try_acquire_lease("p1"), "second acquire refused while held");
        assert!(t.try_acquire_lease("p2"), "other project unaffected");
        t.release_lease("p1");
        assert!(t.try_acquire_lease("p1"), "re-acquirable after release");
        t.release_lease("nonexistent"); // idempotent, no panic
    }

    #[test]
    fn mark_validating_flips_phase_without_touching_counts() {
        let mut t = AuditTracker::default();
        t.start_progress("proj-d", 10, "full_audit");
        t.advance_step("proj-d", 10, Some("Final review".into()));
        t.mark_validating("proj-d");

        let p = t.get_progress("proj-d").unwrap();
        assert_eq!(p.phase, "validating");
        assert_eq!(p.step_index, 10, "mark_validating must not reset counters");
    }

    #[test]
    fn progress_entries_are_isolated_per_project() {
        let mut t = AuditTracker::default();
        t.start_progress("one", 10, "full");
        t.start_progress("two", 5, "partial");
        t.advance_step("one", 7, Some("a.md".into()));

        assert_eq!(t.get_progress("one").unwrap().step_index, 7);
        assert_eq!(t.get_progress("two").unwrap().step_index, 0);
    }
}

#[cfg(test)]
mod auth_tests {
    use super::{auth_allows, is_local_ip};
    use axum::http::Method;

    // ── auth_allows decision matrix (I9 + passe D: destructive-op gating) ────
    const RESET: &str = "/api/setup/reset";
    const IMPORT: &str = "/api/config/import";
    const NORMAL: &str = "/api/projects";

    #[test]
    fn destructive_from_remote_is_denied_even_when_auth_off() {
        // THE fix: Docker default (auth off, 0.0.0.0). A LAN peer — not local,
        // no token — must NOT be able to wipe/overwrite the instance.
        assert!(!auth_allows(&Method::POST, RESET, false, true, false, false));
        assert!(!auth_allows(&Method::POST, IMPORT, false, true, false, false));
        // …and still denied even if no token is configured at all.
        assert!(!auth_allows(&Method::POST, RESET, false, false, false, false));
    }

    #[test]
    fn destructive_allowed_for_local_or_valid_token() {
        assert!(auth_allows(&Method::POST, RESET, false, true, true, false), "local host may reset");
        assert!(auth_allows(&Method::POST, RESET, false, true, false, true), "valid token may reset");
        assert!(auth_allows(&Method::POST, IMPORT, true, true, true, false), "local host may import (auth on)");
    }

    #[test]
    fn recovery_endpoints_are_gated_like_destructive() {
        // recovery/set reads the active key, recovery/restore swaps it — both
        // must be denied to an unauthenticated remote peer even with auth off.
        for p in ["/api/config/recovery/set", "/api/config/recovery/restore"] {
            assert!(!auth_allows(&Method::POST, p, false, true, false, false), "{p} must deny remote+no-token");
            assert!(auth_allows(&Method::POST, p, false, true, true, false), "{p} allows local");
            assert!(auth_allows(&Method::POST, p, false, true, false, true), "{p} allows valid token");
        }
    }

    #[test]
    fn every_delete_is_gated_even_when_auth_off() {
        // Passe D — the VERB is the criterion: a LAN peer on an auth-off bind
        // must not delete projects/discussions/workflows/… whatever the path.
        for p in ["/api/projects/p1", "/api/discussions/d1", "/api/workflow-runs/r1", "/api/anything/new"] {
            assert!(!auth_allows(&Method::DELETE, p, false, true, false, false), "{p} DELETE must deny remote");
            assert!(auth_allows(&Method::DELETE, p, false, true, true, false), "{p} DELETE allows local");
            assert!(auth_allows(&Method::DELETE, p, false, true, false, true), "{p} DELETE allows token");
        }
    }

    #[test]
    fn destructive_posts_inventory_is_gated() {
        // Passe D — system mutations + purges reachable by POST.
        for p in ["/api/audit-runs/cleanup", "/api/api-call-logs/purge",
                  "/api/debug/logs/clear", "/api/agents/uninstall", "/api/rtk/deactivate",
                  "/api/mcps/custom/srv-1/cleanup-orphan-env"] {
            assert!(!auth_allows(&Method::POST, p, false, true, false, false), "{p} must deny remote+no-token");
            assert!(auth_allows(&Method::POST, p, false, true, true, false), "{p} allows local");
        }
    }

    #[test]
    fn non_destructive_keeps_historical_behaviour() {
        // auth off → open (GET and ordinary POST alike).
        assert!(auth_allows(&Method::GET, NORMAL, false, true, false, false));
        assert!(auth_allows(&Method::POST, NORMAL, false, true, false, false));
        // auth on, no token configured → open (first-run/back-compat).
        assert!(auth_allows(&Method::GET, NORMAL, true, false, false, false));
        // auth on, token configured, remote & no token → denied.
        assert!(!auth_allows(&Method::GET, NORMAL, true, true, false, false));
        // auth on, local trusted → open.
        assert!(auth_allows(&Method::GET, NORMAL, true, true, true, false));
        // auth on, valid token → open.
        assert!(auth_allows(&Method::GET, NORMAL, true, true, false, true));
    }

    // Localhost auth-bypass relies entirely on `is_local_ip`. A
    // regression here = either (a) auth incorrectly fires for the
    // user's own machine (annoying), or (b) auth incorrectly
    // bypasses for a remote IP (a real security hole). Pin both.

    #[test]
    fn ipv4_loopback_is_local() {
        assert!(is_local_ip("127.0.0.1"));
    }

    #[test]
    fn ipv6_loopback_is_local() {
        assert!(is_local_ip("::1"));
    }

    #[test]
    fn rejects_localhost_string() {
        // Hostname strings never reach this function in real traffic
        // (axum gives us numeric IPs from SocketAddr). A misconfigured
        // nginx forwarding `X-Real-IP: localhost` was the only way
        // this could fire; we hardened against it on 2026-05-10.
        assert!(!is_local_ip("localhost"));
    }

    #[test]
    fn docker_bridge_range_is_local() {
        // Docker's default bridge subnet is 172.16.0.0/12.
        assert!(is_local_ip("172.16.0.1"));
        assert!(is_local_ip("172.18.0.1"));
        assert!(is_local_ip("172.31.255.254"));
    }

    #[test]
    fn outside_docker_bridge_is_not_local() {
        // 172.0-15 and 172.32-255 are PUBLIC IPs — must not be treated as local.
        assert!(!is_local_ip("172.15.0.1"));
        assert!(!is_local_ip("172.32.0.1"));
        assert!(!is_local_ip("172.100.0.1"));
    }

    #[test]
    fn tailscale_lan_and_public_are_not_local() {
        // Tailscale CGNAT, RFC1918 LAN, and arbitrary public IPs all
        // require Bearer auth. Regression here = a peer can
        // accidentally hit Kronn without a token.
        assert!(!is_local_ip("100.64.0.1")); // Tailscale CGNAT
        assert!(!is_local_ip("192.168.1.10")); // home LAN
        assert!(!is_local_ip("10.0.0.5"));     // RFC1918 LAN
        assert!(!is_local_ip("8.8.8.8"));      // public
    }

    #[test]
    fn malformed_ip_is_not_local() {
        // Defensive: any string that isn't a recognised numeric form
        // = not local. Don't get clever — a parser oddity should
        // fail-closed (require auth).
        assert!(!is_local_ip(""));
        assert!(!is_local_ip("not-an-ip"));
        assert!(!is_local_ip("172.foo.0.1"));
    }
}
