// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use kronn::{
    build_router,
    core::{config, mcp_scanner},
    db::Database,
    models::ApiKey,
    workflows::WorkflowEngine,
    AppState, AuditTracker, DEFAULT_MAX_CONCURRENT_AGENTS,
};

// Embed frontend/dist/ into the binary at compile time.
// This ensures the desktop app works regardless of install location.
use include_dir::{include_dir, Dir};
static FRONTEND_DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../frontend/dist");

/// Find a free TCP port for the embedded backend
fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("Failed to bind to free port");
    listener.local_addr().unwrap().port()
}

/// Extract embedded frontend files to a temp directory for serving.
/// Returns the path to the extracted directory.
fn extract_frontend_dist() -> std::path::PathBuf {
    // In dev mode, try the filesystem path first (faster iteration, no re-extract)
    #[cfg(debug_assertions)]
    {
        let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../frontend/dist");
        if dev_path.join("index.html").exists() {
            tracing::info!("Dev mode: serving frontend from filesystem {:?}", dev_path);
            return dev_path;
        }
    }

    // Production: extract embedded files to a temp directory
    let temp_dir = std::env::temp_dir().join("kronn-desktop-frontend");
    let _ = std::fs::remove_dir_all(&temp_dir); // Clean stale files
    extract_dir(&FRONTEND_DIST, &temp_dir);
    tracing::info!("Extracted frontend dist to {:?}", temp_dir);
    temp_dir
}

/// Recursively extract an embedded directory to the filesystem.
/// `root_target` is always the top-level extraction directory — file.path()
/// returns paths relative to the include_dir root (e.g. "assets/index.js"),
/// so we always join with root_target to avoid doubled paths.
fn extract_dir(dir: &Dir<'_>, root_target: &std::path::Path) {
    for file in dir.files() {
        let path = root_target.join(file.path());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, file.contents()).ok();
    }
    for sub in dir.dirs() {
        extract_dir(sub, root_target);
    }
}

/// Start the Kronn backend server on a given port (runs in a tokio task)
async fn start_backend(port: u16, dist_dir: std::path::PathBuf) -> anyhow::Result<()> {
    tracing::info!("Starting embedded Kronn backend on port {}", port);

    // Load or create config
    let mut app_config = match config::load().await? {
        Some(cfg) => cfg,
        None => config::default_config(),
    };

    // Override server config for embedded mode
    app_config.server.host = "127.0.0.1".to_string();
    app_config.server.port = port;

    let max_agents = if app_config.server.max_concurrent_agents > 0 {
        app_config.server.max_concurrent_agents
    } else {
        DEFAULT_MAX_CONCURRENT_AGENTS
    };

    // Open database
    let database = Arc::new(Database::open().expect("Failed to open database"));

    // Build state
    let (ws_tx, _) = tokio::sync::broadcast::channel(256);
    let config_arc = Arc::new(RwLock::new(app_config));
    let workflow_engine = Arc::new(WorkflowEngine::new(database.clone(), config_arc.clone()));
    let state = AppState {
        config: config_arc,
        db: database,
        workflow_engine: workflow_engine.clone(),
        agent_semaphore: Arc::new(Semaphore::new(max_agents)),
        audit_tracker: Arc::new(std::sync::Mutex::new(AuditTracker::default())),
        ws_broadcast: Arc::new(ws_tx),
    };

    // Auto-discover API keys
    {
        let discovered = kronn::core::key_discovery::discover_keys().await;
        let mut cfg = state.config.write().await;
        let mut imported = 0u32;
        for dk in discovered {
            if !cfg.tokens.keys.iter().any(|k| k.value == dk.value) {
                let is_first = !cfg.tokens.keys.iter().any(|k| k.provider == dk.provider);
                cfg.tokens.keys.push(ApiKey {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: dk.suggested_name.clone(),
                    provider: dk.provider,
                    value: dk.value,
                    active: is_first,
                });
                imported += 1;
            }
        }
        if imported > 0 {
            let _ = config::save(&cfg).await;
            tracing::info!("Auto-imported {} API key(s)", imported);
        }
    }

    // MCP startup sync
    {
        let db = state.db.clone();
        let cfg = state.config.read().await;
        if let Some(secret) = cfg.encryption_secret.clone() {
            drop(cfg);
            let _ = db
                .with_conn(move |conn| {
                    mcp_scanner::sync_all_projects(conn, &secret);
                    Ok(())
                })
                .await;
        }
    }

    // Start workflow engine
    let engine = workflow_engine.clone();
    tokio::spawn(async move { engine.start().await });

    // Start WS client manager for multi-user sync
    let ws_state = state.clone();
    tokio::spawn(async move { kronn::core::ws_client::run(ws_state).await });

    // Build API router
    let api_router = build_router(state);

    // Serve frontend static files + API
    let frontend_service = tower_http::services::ServeDir::new(&dist_dir)
        .append_index_html_on_directories(true);

    // Merge: /api/* → backend, /* → frontend static files
    let app = axum::Router::new()
        .merge(api_router)
        .fallback_service(frontend_service)
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::HeaderName::from_static("cross-origin-opener-policy"),
                axum::http::HeaderValue::from_static("same-origin"),
            ),
        )
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::overriding(
                axum::http::HeaderName::from_static("cross-origin-embedder-policy"),
                axum::http::HeaderValue::from_static("require-corp"),
            ),
        );

    let addr = format!("127.0.0.1:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Kronn ready on http://{}", addr);

    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await?;
    Ok(())
}

struct BackendInfo {
    port: u16,
}

#[tauri::command]
fn get_backend_url(info: tauri::State<'_, BackendInfo>) -> String {
    format!("http://127.0.0.1:{}", info.port)
}

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_writer(std::io::stdout)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kronn=info".into()),
        )
        .init();

    let port = find_free_port();

    // Extract frontend dist (embedded in binary for production, filesystem for dev)
    let dist_dir = extract_frontend_dist();

    // Start the backend in a background thread with its own tokio runtime
    let backend_port = port;
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");
        rt.block_on(async {
            if let Err(e) = start_backend(backend_port, dist_dir).await {
                tracing::error!("Backend failed: {}", e);
            }
        });
    });

    // Wait for backend to be ready (TCP check).
    // First launch on Windows can be slow (Defender scan, DB creation, key discovery).
    for i in 0..150 {
        // 150 × 100ms = 15 seconds max
        if std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
            tracing::info!("Backend ready after {}ms", i * 100);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Launch Tauri app — webview loads from the backend HTTP server (not custom protocol)
    // This ensures SharedArrayBuffer is available for WASM threading (TTS/STT)
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(BackendInfo { port })
        .invoke_handler(tauri::generate_handler![get_backend_url])
        .setup(move |app| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                let url: tauri::Url = format!("http://127.0.0.1:{}", port)
                    .parse()
                    .expect("Invalid URL");
                let _ = window.navigate(url);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Error while running Kronn Desktop");
}
