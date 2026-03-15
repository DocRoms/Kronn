use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use kronn::{build_router, core::config, db::Database, workflows::WorkflowEngine, AppState, AuditTracker, DEFAULT_MAX_CONCURRENT_AGENTS};

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
    // In Docker (KRONN_DATA_DIR is set), always bind to 0.0.0.0
    // so nginx in its own container can reach us.
    let host = if std::env::var("KRONN_DATA_DIR").is_ok() {
        "0.0.0.0".to_string()
    } else {
        app_config.server.host.clone()
    };
    let max_agents = if app_config.server.max_concurrent_agents > 0 {
        app_config.server.max_concurrent_agents
    } else {
        DEFAULT_MAX_CONCURRENT_AGENTS
    };

    // Log auth status
    if app_config.server.auth_token.is_some() {
        tracing::info!("API authentication enabled (Bearer token)");
    } else {
        tracing::warn!("API authentication disabled — API is open to anyone on the network");
    }

    // Open database
    let database = Arc::new(Database::open().expect("Failed to open database"));
    tracing::info!("Database opened at {}/kronn.db", config::config_dir().unwrap().display());

    // Build state
    let config_arc = Arc::new(RwLock::new(app_config));
    let workflow_engine = Arc::new(WorkflowEngine::new(database.clone(), config_arc.clone()));
    let state = AppState {
        config: config_arc,
        db: database,
        workflow_engine: workflow_engine.clone(),
        agent_semaphore: Arc::new(Semaphore::new(max_agents)),
        audit_tracker: Arc::new(std::sync::Mutex::new(AuditTracker::default())),
    };

    // Start workflow engine in background
    let engine = workflow_engine.clone();
    tokio::spawn(async move { engine.start().await });

    // Build router
    let app = build_router(state);

    // Start server
    let addr = format!("{}:{}", host, port);
    tracing::info!("Listening on {}", addr);
    println!();
    println!("  ╔═══════════════════════════════════════╗");
    println!("  ║                                       ║");
    println!("  ║   K R O N N   v0.1.0                  ║");
    println!("  ║   ─────────────────                   ║");
    println!("  ║   Entering the grid...                ║");
    println!("  ║                                       ║");
    println!("  ║   → http://{}:{:<13} ║", host, port);
    println!("  ║   Agents: max {} concurrent          ║", max_agents);
    println!("  ║                                       ║");
    println!("  ╚═══════════════════════════════════════╝");
    println!();

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Graceful shutdown: wait for SIGTERM/SIGINT, then let in-flight requests finish
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Kronn — Shutdown complete.");
    Ok(())
}

/// Wait for SIGTERM or SIGINT (Ctrl+C / Docker stop).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("Received Ctrl+C, shutting down gracefully..."),
        _ = terminate => tracing::info!("Received SIGTERM, shutting down gracefully..."),
    }
}
