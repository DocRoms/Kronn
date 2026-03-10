use std::sync::Arc;
use tokio::sync::RwLock;

use kronn::{build_router, core::config, db::Database, workflows::WorkflowEngine, AppState};

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
    let config_arc = Arc::new(RwLock::new(app_config));
    let workflow_engine = Arc::new(WorkflowEngine::new(database.clone(), config_arc.clone()));
    let state = AppState {
        config: config_arc,
        db: database,
        workflow_engine: workflow_engine.clone(),
    };

    // Start workflow engine in background
    let engine = workflow_engine.clone();
    tokio::spawn(async move { engine.start().await });

    // Build router
    let app = build_router(state);

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
