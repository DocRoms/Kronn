pub mod api;
pub mod agents;
pub mod core;
pub mod db;
pub mod models;
pub mod workflows;

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

/// Tracks running audit processes so they can be cancelled.
#[derive(Default)]
pub struct AuditTracker {
    /// Currently running child PID per project (if any)
    pub running_pids: HashMap<String, u32>,
    /// Projects whose audit should be cancelled
    pub cancelled: HashSet<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub db: Arc<Database>,
    pub workflow_engine: Arc<WorkflowEngine>,
    pub agent_semaphore: Arc<Semaphore>,
    pub audit_tracker: Arc<Mutex<AuditTracker>>,
}

// ─── Auth Middleware ─────────────────────────────────────────────────────────

/// Bearer token authentication middleware.
/// Skips auth for /api/health (Docker healthcheck) and when no token is configured.
async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, StatusCode> {
    // Skip auth for health endpoint
    if request.uri().path() == "/api/health" {
        return Ok(next.run(request).await);
    }

    let config = state.config.read().await;
    let expected_token = config.server.auth_token.clone();
    drop(config);

    // If no token is configured, skip auth (backward compat / first run)
    let Some(expected) = expected_token else {
        return Ok(next.run(request).await);
    };

    // Check Authorization: Bearer <token>
    let authorized = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|token| token == expected)
        .unwrap_or(false);

    if authorized {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
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
        let config = state.config.try_read().expect("Config lock poisoned at startup");
        (config.server.domain.clone(), config.server.port)
    };

    let mut router = Router::new()
        // ── Health (lightweight, used by Docker healthcheck — no auth) ──
        .route("/api/health", get(|| async { axum::Json(serde_json::json!({"ok": true})) }))
        // ── Setup wizard ──
        .route("/api/setup/status", get(api::setup::get_status))
        .route("/api/setup/scan-paths", post(api::setup::set_scan_paths))
        .route("/api/setup/install-agent", post(api::setup::install_agent))
        .route("/api/setup/complete", post(api::setup::complete))
        .route("/api/setup/reset", post(api::setup::reset))
        // ── Config ──
        .route("/api/config/tokens", get(api::setup::get_tokens))
        .route("/api/config/api-keys", post(api::setup::save_api_key))
        .route("/api/config/api-keys/:id", delete(api::setup::delete_api_key))
        .route("/api/config/api-keys/:id/activate", post(api::setup::activate_api_key))
        .route("/api/config/sync-agent-tokens", post(api::setup::sync_agent_tokens))
        .route("/api/config/discover-keys", post(api::setup::discover_keys))
        .route("/api/config/toggle-token-override", post(api::setup::toggle_token_override))
        .route("/api/config/language", get(api::setup::get_language).post(api::setup::save_language))
        .route("/api/config/scan-paths", get(api::setup::get_scan_paths).post(api::setup::set_scan_paths))
        .route("/api/config/scan-ignore", get(api::setup::get_scan_ignore).post(api::setup::set_scan_ignore))
        .route("/api/config/scan-depth", get(api::setup::get_scan_depth).post(api::setup::set_scan_depth))
        .route("/api/config/agent-access", get(api::setup::get_agent_access).post(api::setup::set_agent_access))
        .route("/api/config/server", get(api::setup::get_server_config).post(api::setup::set_server_config))
        .route("/api/config/auth-token/regenerate", post(api::setup::regenerate_auth_token))
        .route("/api/config/db-info", get(api::setup::db_info))
        .route("/api/config/export", get(api::setup::export_data))
        .route("/api/config/import", post(api::setup::import_data))
        // ── Projects ──
        .route("/api/projects", get(api::projects::list))
        .route("/api/projects", post(api::projects::create))
        .route("/api/projects/scan", post(api::projects::scan))
        .route("/api/projects/bootstrap", post(api::projects::bootstrap))
        .route("/api/projects/clone", post(api::projects::clone_project))
        .route("/api/projects/discover-repos", post(api::projects::discover_repos))
        .route("/api/projects/:id", get(api::projects::get))
        .route("/api/projects/:id", delete(api::projects::delete))
        .route("/api/projects/:id/install-template", post(api::projects::install_template))
        .route("/api/projects/:id/ai-audit", post(api::projects::run_audit))
        .route("/api/projects/:id/audit-info", get(api::projects::audit_info))
        .route("/api/projects/:id/validate-audit", post(api::projects::validate_audit))
        .route("/api/projects/:id/mark-bootstrapped", post(api::projects::mark_bootstrapped))
        .route("/api/projects/:id/full-audit", post(api::projects::full_audit))
        .route("/api/projects/:id/cancel-audit", post(api::projects::cancel_audit))
        .route("/api/projects/:id/default-skills", put(api::projects::set_default_skills))
        .route("/api/projects/:id/default-profile", put(api::projects::set_default_profile))
        .route("/api/projects/:id/ai-files", get(api::projects::list_ai_files))
        .route("/api/projects/:id/ai-file", get(api::projects::read_ai_file))
        .route("/api/projects/:id/ai-search", get(api::projects::search_ai_files))
        .route("/api/projects/:id/git-status", get(api::projects::git_status))
        .route("/api/projects/:id/git-diff", get(api::projects::git_diff))
        .route("/api/projects/:id/git-branch", post(api::projects::git_branch))
        .route("/api/projects/:id/git-commit", post(api::projects::git_commit))
        .route("/api/projects/:id/git-push", post(api::projects::git_push))
        .route("/api/projects/:id/exec", post(api::projects::project_exec))
        // ── Agents ──
        .route("/api/agents", get(api::agents::detect))
        .route("/api/agents/install", post(api::agents::install))
        .route("/api/agents/uninstall", post(api::agents::uninstall))
        .route("/api/agents/toggle", post(api::agents::toggle))
        // ── MCPs ──
        .route("/api/mcps", get(api::mcps::overview))
        .route("/api/mcps/registry", get(api::mcps::list_registry))
        .route("/api/mcps/refresh", post(api::mcps::refresh))
        .route("/api/mcps/configs", post(api::mcps::create_config))
        .route("/api/mcps/configs/:id", patch(api::mcps::update_config).delete(api::mcps::delete_config))
        .route("/api/mcps/configs/:id/projects", patch(api::mcps::set_config_projects))
        .route("/api/mcps/configs/:id/reveal", post(api::mcps::reveal_secrets))
        .route("/api/mcps/context/:project_id", get(api::mcps::list_contexts))
        .route("/api/mcps/context/:project_id/:slug", get(api::mcps::get_context).put(api::mcps::update_context))
        // ── Workflows ──
        .route("/api/workflows", get(api::workflows::list).post(api::workflows::create))
        .route("/api/workflows/:id", get(api::workflows::get).put(api::workflows::update).delete(api::workflows::delete))
        .route("/api/workflows/:id/trigger", post(api::workflows::trigger))
        .route("/api/workflows/:id/runs", get(api::workflows::list_runs).delete(api::workflows::delete_all_runs))
        .route("/api/workflows/:id/runs/:run_id", get(api::workflows::get_run).delete(api::workflows::delete_run))
        // ── Discussions ──
        .route("/api/discussions", get(api::discussions::list))
        .route("/api/discussions", post(api::discussions::create))
        .route("/api/discussions/:id", get(api::discussions::get))
        .route("/api/discussions/:id", delete(api::discussions::delete).patch(api::discussions::update))
        .route("/api/discussions/:id/messages", post(api::discussions::send_message))
        .route("/api/discussions/:id/messages/last", delete(api::discussions::delete_last_agent_messages).patch(api::discussions::edit_last_user_message))
        .route("/api/discussions/:id/run", post(api::discussions::run_agent))
        .route("/api/discussions/:id/orchestrate", post(api::discussions::orchestrate))
        // ── Skills ──
        .route("/api/skills", get(api::skills::list).post(api::skills::create))
        .route("/api/skills/:id", put(api::skills::update).delete(api::skills::delete))
        // ── Profiles ──
        .route("/api/profiles", get(api::profiles::list).post(api::profiles::create))
        .route("/api/profiles/:id", get(api::profiles::get).put(api::profiles::update).delete(api::profiles::delete))
        .route("/api/profiles/:id/persona-name", put(api::profiles::update_persona_name))
        // ── Directives ──
        .route("/api/directives", get(api::directives::list).post(api::directives::create))
        .route("/api/directives/:id", put(api::directives::update).delete(api::directives::delete))
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
