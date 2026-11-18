//! Process-wide application state shared by every HTTP handler and background
//! task, kept apart from the router so lower crates can name it.

use crate::db::Database;
use crate::models::AppConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::{RwLock, Semaphore};

// ─── Application State ──────────────────────────────────────────────────────

/// Default maximum concurrent agent processes.
pub const DEFAULT_MAX_CONCURRENT_AGENTS: usize = 5;

/// Worker stack size. In a debug build one nested workflow run costs about
/// 1 MiB of stack, so a foreach child overflowed the 2 MiB default (KT-1161).
pub const RUNTIME_THREAD_STACK_SIZE: usize = 8 << 20;

/// The multi-threaded runtime the backend runs on, shared by the server and
/// desktop entry points so both get the same worker configuration.
pub fn runtime_builder() -> tokio::runtime::Builder {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .enable_all()
        .thread_stack_size(RUNTIME_THREAD_STACK_SIZE);
    builder
}

/// Tracks running audit processes so they can be cancelled AND inspected.
///
/// `progress` is the data source for `GET /api/projects/:id/audit-status` —
/// the UI polls it to resume the progress bar after tab/page navigation.
/// Entries are inserted by the SSE streams (`full_audit`, `partial_audit`)
/// and removed on completion/cancel/error.
#[derive(Default)]
pub struct AuditTracker {
    /// Currently running child PID per project (if any). Only a direct CLI agent
    /// is registered here: the PID of an HTTP or ACP agent is a lifeline, not
    /// the agent.
    pub running_pids: HashMap<String, u32>,
    /// Cancellation of the step's agent per project, for an agent that does not
    /// run as the process in `running_pids`: an HTTP agent lives in a task
    /// (KT-924), an ACP agent — OpenCode and the others, Claude and Codex
    /// through their adapters — in a session Kronn cancels and whose process it
    /// then shuts down (KT-927). Killing the lifeline alone would leave either
    /// reading and writing files.
    pub agent_cancels: HashMap<String, tokio_util::sync::CancellationToken>,
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
    /// Leases whose run could not be settled (the drop guard's database write
    /// failed): kept fail-closed, and named so the UI offers the cleanup that
    /// releases them (KT-1134).
    pub orphaned_leases: HashSet<String>,
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
        self.orphaned_leases.remove(project_id);
    }

    /// Whether the project's lease is held by a run nobody can settle.
    pub fn lease_orphaned(&self, project_id: &str) -> bool {
        self.leased.contains(project_id) && self.orphaned_leases.contains(project_id)
    }

    /// Seed progress when an audit stream starts. Called from the `start`
    /// SSE event. Resets any stale progress row for the same project.
    pub fn start_progress(&mut self, project_id: impl Into<String>, total_steps: u32, kind: &str) {
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
                agent: None,
                tier: None,
                connection_id: None,
                recent_activity: None,
                step_breakdown: None,
            },
        );
    }

    /// Record who runs the audit, for any client polling its progress.
    pub fn set_auditor(
        &mut self,
        project_id: &str,
        agent: crate::models::AgentType,
        tier: crate::models::ModelTier,
        connection_id: Option<String>,
    ) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.agent = Some(agent);
            entry.tier = Some(tier);
            entry.connection_id = connection_id;
        }
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
            if let Some(s) = step_tokens {
                entry.step_tokens = Some(s);
            }
            if let Some(t) = total_tokens_so_far {
                entry.total_tokens_so_far = Some(t);
            }
            if let Some(tool) = current_tool {
                entry.current_tool = Some(tool);
                // 0.8.4 (#319 / B3) — bump the tool-call counter for
                // the live "still alive" chip. Every tool_call from
                // the agent stream lands here, regardless of whether
                // the agent also emitted a `Usage` block. Reset on
                // step boundary via `clear_step_chips`.
                entry.current_tool_call_count =
                    Some(entry.current_tool_call_count.unwrap_or(0) + 1);
            }
        }
    }

    /// The running step's headline parts, read by a client polling the progress.
    pub fn set_step_breakdown(
        &mut self,
        project_id: &str,
        breakdown: crate::models::AuditTokenBreakdown,
    ) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.step_breakdown = Some(breakdown);
        }
    }

    /// An HTTP agent's tool activity: it reports its last tool and running
    /// count rather than one event per call, so both are set, not bumped.
    pub fn set_tool_activity(&mut self, project_id: &str, tool: String, calls: u32) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.current_tool = Some(tool);
            entry.current_tool_call_count = Some(calls);
        }
    }

    /// The running step's latest actions, replaced whole: the step's feed owns
    /// the bounds and the sanitizing.
    pub fn set_recent_activity(
        &mut self,
        project_id: &str,
        recent: crate::models::AuditRecentActivity,
    ) {
        if let Some(entry) = self.progress.get_mut(project_id) {
            entry.recent_activity = Some(recent);
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
            entry.recent_activity = None;
            entry.step_breakdown = None;
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

/// Runtime switch of P2P federation, apart from the config lock (KT-1033).
/// Federation checks take the read side for a short critical section (never
/// across an await); turning P2P off takes the write side, so it waits for
/// those in progress and none starts after.
#[derive(Debug, Default)]
pub struct P2pGate(std::sync::RwLock<bool>);

impl P2pGate {
    pub fn new(enabled: bool) -> Self {
        Self(std::sync::RwLock::new(enabled))
    }

    pub fn enabled(&self) -> bool {
        *self.read()
    }

    /// Hold while checking and acting; the value cannot turn off meanwhile.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, bool> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn set(&self, enabled: bool) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = enabled;
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    /// P2P federation on/off, mirrored from `server.p2p_enabled`.
    pub p2p: Arc<P2pGate>,
    pub db: Arc<Database>,
    pub agent_semaphore: Arc<Semaphore>,
    pub audit_tracker: Arc<Mutex<AuditTracker>>,
    /// Broadcast channel for WebSocket messages (presence, heartbeat, chat, batch progress).
    pub ws_broadcast: Arc<tokio::sync::broadcast::Sender<crate::models::WsMessage>>,
    /// Registry of in-flight cancellable tasks keyed by a string id — used by
    /// the "⏹ Arrêter" UI to abort a running agent discussion or workflow
    /// run. Keys are a durable agent-dispatch id, a `disc_id` for legacy
    /// non-dispatch streams, or a workflow `run_id`. Tokens are inserted when
    /// work starts and removed in its finally-block — see the Registry impl below.
    pub cancel_registry: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// Dispatches whose provider runtime has been successfully created and has
    /// not yet returned. Cancellation registration happens earlier, so it must
    /// not be used as process-liveness evidence.
    pub agent_runtime_registry: Arc<Mutex<HashSet<String>>>,
    /// KT-1108 — the discussion runs in progress, for a page that opens mid-run.
    pub live_runs: Arc<crate::agents::run_progress::LiveRuns>,
    /// Task executions with a `/resume` in flight. Boot resumes run once HTTP is
    /// served, so a concurrent resume of the same execution is refused.
    pub execution_resumes: Arc<Mutex<HashSet<String>>>,
    /// OAuth2 access-token cache for API plugins. Keyed by `mcp_configs.id`,
    /// value is the bearer token + its absolute expiry. In-memory only —
    /// on restart, tokens are lost and re-exchanged on first use (one HTTP
    /// call per active OAuth2 plugin). See `core::oauth2_cache`.
    pub oauth2_cache:
        Arc<tokio::sync::Mutex<HashMap<String, crate::core::oauth2_cache::CachedToken>>>,
    /// Dependency update checks may hit remote registries, so project
    /// overviews reuse results until a manifest changes or the cache expires.
    pub dependency_update_cache: Arc<
        tokio::sync::Mutex<
            HashMap<String, crate::core::dependency_updates::CachedDependencyUpdates>,
        >,
    >,
    /// Cached project language breakdowns. Git status itself stays live; only
    /// the bounded source scan is reused between overview visits.
    pub git_language_cache:
        Arc<tokio::sync::Mutex<HashMap<String, crate::models::CachedProjectLanguages>>>,
    pub agent_dispatch_notify: Arc<tokio::sync::Notify>,
    /// Handle to the kronn-docs Python sidecar (PDF/DOCX/XLSX/… gen).
    /// Always allocated — whether the sidecar is actually running is
    /// stored INSIDE the handle. Routes in `api::docs` probe it at
    /// call time and return a 503 if the bundled/runtime sidecar could
    /// not be started.
    pub docs_sidecar: Arc<crate::core::docs_sidecar::DocsSidecar>,
    /// Invocation-local discovery endpoint for isolated routers. Production
    /// leaves it unset and retains OLLAMA_HOST/Docker resolution.
    pub ollama_base_url_override: Option<Arc<str>>,
    /// Invocation-local source of the official library's manifests (KT-930),
    /// so a test never reaches the real registry. Production leaves it unset
    /// and uses the process-wide one.
    pub ollama_registry_override: Option<Arc<crate::core::ollama_registry::OllamaRegistry>>,
    /// Production-only data-directory lock. Spawned Git commits inherit a
    /// duplicate of this handle so a replacement backend waits for Git/hooks.
    pub data_dir_lock: Option<Arc<std::fs::File>>,
    /// Workflow Agent steps running with a room capability (KT-793).
    pub workflow_step_rooms: Arc<crate::workflows::step_room::WorkflowStepRooms>,
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
        // Nothing else holds the fresh config yet.
        let (p2p_enabled, key) = config
            .try_read()
            .map(|config| (config.server.p2p_enabled, config.encryption_secret.clone()))
            .unwrap_or((false, None));
        db.assistant_guard().set_key(key);
        let ws_broadcast = Arc::new(ws_tx);
        let live_runs: Arc<crate::agents::run_progress::LiveRuns> = Arc::default();
        Self {
            p2p: Arc::new(P2pGate::new(p2p_enabled)),
            config,
            db,
            agent_semaphore: Arc::new(Semaphore::new(max_agents)),
            audit_tracker: Arc::new(Mutex::new(AuditTracker::default())),
            ws_broadcast: ws_broadcast.clone(),
            cancel_registry: Arc::new(Mutex::new(HashMap::new())),
            agent_runtime_registry: Arc::new(Mutex::new(HashSet::new())),
            live_runs: live_runs.clone(),
            execution_resumes: Arc::new(Mutex::new(HashSet::new())),
            oauth2_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            dependency_update_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            git_language_cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            agent_dispatch_notify: Arc::new(tokio::sync::Notify::new()),
            docs_sidecar: Arc::new(crate::core::docs_sidecar::DocsSidecar::new()),
            ollama_base_url_override: None,
            ollama_registry_override: None,
            data_dir_lock: None,
            workflow_step_rooms: Arc::new(
                crate::workflows::step_room::WorkflowStepRooms::publishing(ws_broadcast, live_runs),
            ),
        }
    }

    pub fn with_data_dir_lock(mut self, data_dir_lock: std::fs::File) -> Self {
        self.data_dir_lock = Some(Arc::new(data_dir_lock));
        self
    }

    pub fn with_ollama_base_url(mut self, base_url: impl Into<Arc<str>>) -> Self {
        self.ollama_base_url_override = Some(base_url.into());
        self
    }

    pub fn with_ollama_registry(
        mut self,
        registry: Arc<crate::core::ollama_registry::OllamaRegistry>,
    ) -> Self {
        self.ollama_registry_override = Some(registry);
        self
    }
}

pub struct AgentRuntimeGuard {
    registry: Arc<Mutex<HashSet<String>>>,
    key: String,
}

impl AgentRuntimeGuard {
    pub fn insert(registry: &Arc<Mutex<HashSet<String>>>, key: impl Into<String>) -> Self {
        let key = key.into();
        if let Ok(mut entries) = registry.lock() {
            entries.insert(key.clone());
        }
        Self {
            registry: registry.clone(),
            key,
        }
    }
}

impl Drop for AgentRuntimeGuard {
    fn drop(&mut self) {
        if let Ok(mut entries) = self.registry.lock() {
            entries.remove(&self.key);
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
    /// Atomically reserve `key` only when no run is already registered.
    ///
    /// Peer-agent messages can arrive concurrently. Their native-run
    /// dispatcher uses this variant so two nearly-simultaneous replies do
    /// not start two local agents for the same discussion.
    pub fn try_insert(
        registry: &Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
        key: impl Into<String>,
    ) -> Option<Self> {
        let key = key.into();
        let token = tokio_util::sync::CancellationToken::new();
        let mut map = registry.lock().ok()?;
        if map.contains_key(&key) {
            return None;
        }
        map.insert(key.clone(), token.clone());
        drop(map);
        Some(Self {
            registry: registry.clone(),
            key,
            token,
        })
    }

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
        Self {
            registry: registry.clone(),
            key,
            token,
        }
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
