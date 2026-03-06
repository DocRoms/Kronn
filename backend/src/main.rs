mod api;
mod agents;
mod core;
mod db;
mod models;
mod scheduler;
mod workflows;

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

use crate::core::config;
use crate::db::Database;
use crate::models::AppConfig;
use crate::scheduler::Scheduler;
use crate::workflows::WorkflowEngine;

// ─── Application State ──────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub db: Arc<Database>,
    pub scheduler: Arc<Scheduler>,
    pub workflow_engine: Arc<WorkflowEngine>,
}

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kronn=info,tower_http=info".into()),
        )
        .init();

    tracing::info!("Kronn — Entering the grid...");

    // Load or create config
    let app_config = match config::load().await? {
        Some(cfg) => {
            tracing::info!("Config loaded from {}", config::config_path()?.display());
            cfg
        }
        None => {
            tracing::info!("First run detected — setup wizard will guide you");
            config::default_config()
        }
    };

    let port = app_config.server.port;

    // Open database
    let database = Arc::new(Database::open().expect("Failed to open database"));
    tracing::info!("Database opened at {}/kronn.db", config::config_dir().unwrap().display());

    // Build state
    let scheduler = Arc::new(Scheduler::new());
    let config_arc = Arc::new(RwLock::new(app_config));
    let workflow_engine = Arc::new(WorkflowEngine::new(database.clone(), config_arc.clone()));
    let state = AppState {
        config: config_arc,
        db: database,
        scheduler: scheduler.clone(),
        workflow_engine: workflow_engine.clone(),
    };

    // Start scheduler in background (legacy)
    let sched = scheduler.clone();
    tokio::spawn(async move { sched.start().await });

    // Start workflow engine in background
    let engine = workflow_engine.clone();
    tokio::spawn(async move { engine.start().await });

    // Build router
    let app = Router::new()
        // ── Setup wizard ──
        .route("/api/setup/status", get(api::setup::get_status))
        .route("/api/setup/scan-paths", post(api::setup::set_scan_paths))
        .route("/api/setup/install-agent", post(api::setup::install_agent))
        .route("/api/setup/complete", post(api::setup::complete))
        .route("/api/setup/reset", post(api::setup::reset))
        // ── Config ──
        .route("/api/config/tokens", get(api::setup::get_tokens).post(api::setup::save_tokens))
        .route("/api/config/language", get(api::setup::get_language).post(api::setup::save_language))
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
        // ── Agents ──
        .route("/api/agents", get(api::agents::detect))
        .route("/api/agents/install", post(api::agents::install))
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
        // ── Tasks (legacy) ──
        .route("/api/projects/:project_id/tasks", get(api::tasks::list))
        .route("/api/projects/:project_id/tasks", post(api::tasks::create))
        .route("/api/projects/:project_id/tasks/:task_id", delete(api::tasks::delete))
        .route("/api/projects/:project_id/tasks/:task_id/toggle", patch(api::tasks::toggle))
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
        .route("/api/discussions/:id", delete(api::discussions::delete))
        .route("/api/discussions/:id/messages", post(api::discussions::send_message))
        .route("/api/discussions/:id/messages/last", delete(api::discussions::delete_last_agent_messages).patch(api::discussions::edit_last_user_message))
        .route("/api/discussions/:id/run", post(api::discussions::run_agent))
        .route("/api/discussions/:id/orchestrate", post(api::discussions::orchestrate))
        // ── Stats ──
        .route("/api/stats/tokens", get(api::stats::token_usage))
        // ── Middleware ──
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = format!("0.0.0.0:{}", port);
    tracing::info!("Listening on {}", addr);
    println!();
    println!("  ╔═══════════════════════════════════════╗");
    println!("  ║                                       ║");
    println!("  ║   K R O N N   v0.1.0                  ║");
    println!("  ║   ─────────────────                   ║");
    println!("  ║   Entering the grid...                ║");
    println!("  ║                                       ║");
    println!("  ║   → http://localhost:{:<16} ║", port);
    println!("  ║                                       ║");
    println!("  ╚═══════════════════════════════════════╝");
    println!();

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
