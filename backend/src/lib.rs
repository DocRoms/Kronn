pub mod api;
pub mod agents;
pub mod core;
pub mod db;
pub mod models;
pub mod workflows;

use std::sync::Arc;
use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};
use tokio::sync::RwLock;
use tower_http::{
    cors::CorsLayer,
    trace::TraceLayer,
};

pub use crate::db::Database;
pub use crate::models::AppConfig;
pub use crate::workflows::WorkflowEngine;

// ─── Application State ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub db: Arc<Database>,
    pub workflow_engine: Arc<WorkflowEngine>,
}

/// Build the Axum router with all routes and middleware.
/// Extracted for reuse in integration tests.
pub fn build_router(state: AppState) -> Router {
    Router::new()
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
        .route("/api/config/db-info", get(api::setup::db_info))
        .route("/api/config/export", get(api::setup::export_data))
        .route("/api/config/import", post(api::setup::import_data))
        // ── Projects ──
        .route("/api/projects", get(api::projects::list))
        .route("/api/projects", post(api::projects::create))
        .route("/api/projects/scan", post(api::projects::scan))
        .route("/api/projects/:id", get(api::projects::get))
        .route("/api/projects/:id", delete(api::projects::delete))
        .route("/api/projects/:id/install-template", post(api::projects::install_template))
        .route("/api/projects/:id/ai-audit", post(api::projects::run_audit))
        .route("/api/projects/:id/validate-audit", post(api::projects::validate_audit))
        .route("/api/projects/:id/default-skills", put(api::projects::set_default_skills))
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
        // ── Stats ──
        .route("/api/stats/tokens", get(api::stats::token_usage))
        .route("/api/stats/agent-usage", get(api::stats::agent_usage))
        // ── Middleware ──
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
