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

// ── Wake lock ──────────────────────────────────────────────────────────────

mod wake_lock {
    use std::sync::atomic::{AtomicBool, Ordering};

    static ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn is_active() -> bool {
        ACTIVE.load(Ordering::Relaxed)
    }

    /// Acquire a wake lock — prevents the OS from sleeping.
    pub fn acquire() {
        if ACTIVE.swap(true, Ordering::SeqCst) {
            return; // Already active
        }
        tracing::info!("Wake lock acquired — preventing system sleep");
        #[cfg(target_os = "windows")]
        unsafe {
            // ES_CONTINUOUS | ES_SYSTEM_REQUIRED — prevent sleep, allow screen off
            windows_set_execution_state(0x80000001 | 0x00000001);
        }
        #[cfg(target_os = "macos")]
        {
            // Spawn caffeinate in background — it will be killed on release
            std::thread::spawn(|| {
                let _ = std::process::Command::new("caffeinate")
                    .arg("-i") // prevent idle sleep
                    .arg("-w")
                    .arg(std::process::id().to_string()) // tied to this process
                    .spawn();
            });
        }
        // Linux: systemd-inhibit or similar — most Linux desktops don't auto-sleep
    }

    /// Release the wake lock — allow the OS to sleep again.
    pub fn release() {
        if !ACTIVE.swap(false, Ordering::SeqCst) {
            return; // Already released
        }
        tracing::info!("Wake lock released — system can sleep");
        #[cfg(target_os = "windows")]
        unsafe {
            // ES_CONTINUOUS only — restore normal sleep behavior
            windows_set_execution_state(0x80000001);
        }
        // macOS: caffeinate was tied to our PID with -w, it self-terminates
    }

    #[cfg(target_os = "windows")]
    unsafe fn windows_set_execution_state(flags: u32) {
        #[link(name = "kernel32")]
        extern "system" {
            fn SetThreadExecutionState(esFlags: u32) -> u32;
        }
        SetThreadExecutionState(flags);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn acquire_sets_active() {
            // Reset state
            ACTIVE.store(false, Ordering::SeqCst);
            assert!(!is_active());
            acquire();
            assert!(is_active());
        }

        #[test]
        fn release_clears_active() {
            ACTIVE.store(true, Ordering::SeqCst);
            release();
            assert!(!is_active());
        }

        #[test]
        fn double_acquire_is_idempotent() {
            ACTIVE.store(false, Ordering::SeqCst);
            acquire();
            acquire(); // Should not panic or double-lock
            assert!(is_active());
            release();
            assert!(!is_active());
        }

        #[test]
        fn double_release_is_idempotent() {
            ACTIVE.store(false, Ordering::SeqCst);
            release();
            release(); // Should not panic
            assert!(!is_active());
        }
    }
}

// ── Wake lock watcher ──────────────────────────────────────────────────────

/// Periodically check if any cron workflows are enabled and toggle the wake lock.
async fn wake_lock_watcher(db: Arc<Database>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
    loop {
        interval.tick().await;
        let has_cron = db
            .with_conn(|conn| {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM workflows WHERE enabled = 1 AND json_extract(trigger_json, '$.type') = 'Cron'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                Ok(count > 0)
            })
            .await
            .unwrap_or(false);

        if has_cron && !wake_lock::is_active() {
            wake_lock::acquire();
        } else if !has_cron && wake_lock::is_active() {
            wake_lock::release();
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

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

// ── PATH enrichment for desktop apps ───────────────────────────────────────

/// GUI apps on macOS inherit a minimal PATH (/usr/bin:/bin:/usr/sbin:/sbin).
/// Shell-installed tools (npm global, homebrew, cargo, pip, etc.) are invisible.
/// This adds common installation directories to PATH so agent detection works.
fn enrich_path() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let extra_dirs: Vec<String> = vec![
        // npm global (macOS/Linux)
        format!("{}/.local/bin", home),
        format!("{}/.npm-global/bin", home),
        // Homebrew (macOS)
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
        // Cargo (Rust)
        format!("{}/.cargo/bin", home),
        // Python / pip
        format!("{}/Library/Python/3.11/bin", home),
        format!("{}/Library/Python/3.12/bin", home),
        format!("{}/.local/share/pipx/venvs/bin", home),
        // uv (Python)
        format!("{}/.local/share/uv/bin", home),
        // Node via nvm / fnm
        format!("{}/.nvm/current/bin", home),
        format!("{}/.fnm/current/bin", home),
        // Bun
        format!("{}/.bun/bin", home),
    ];

    let current_path = std::env::var("PATH").unwrap_or_default();
    let mut paths: Vec<&str> = current_path.split(':').collect();

    let mut added = 0;
    for dir in &extra_dirs {
        if !paths.contains(&dir.as_str()) && std::path::Path::new(dir).is_dir() {
            paths.push(dir);
            added += 1;
        }
    }

    if added > 0 {
        let new_path = paths.join(":");
        std::env::set_var("PATH", &new_path);
        tracing::info!("Enriched PATH with {} additional directories", added);
    }
}

// ── Backend ────────────────────────────────────────────────────────────────

/// Start the Kronn backend server on a given port (runs in a tokio task)
async fn start_backend(port: u16, dist_dir: std::path::PathBuf) -> anyhow::Result<()> {
    tracing::info!("Starting embedded Kronn backend on port {}", port);

    // Enrich PATH for desktop mode — GUI apps on macOS/Linux inherit a minimal PATH
    // that doesn't include user-installed binaries (npm global, homebrew, cargo, etc.)
    enrich_path();

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
        db: database.clone(),
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

    // Start wake lock watcher (toggles OS wake lock based on active cron workflows)
    tokio::spawn(wake_lock_watcher(database));

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

// ── Main ───────────────────────────────────────────────────────────────────

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

            // Navigate main window to backend URL
            if let Some(window) = app.get_webview_window("main") {
                let url: tauri::Url = format!("http://127.0.0.1:{}", port)
                    .parse()
                    .expect("Invalid URL");
                let _ = window.navigate(url);
            }

            // ── System tray menu ──
            use tauri::menu::{MenuBuilder, MenuItemBuilder};
            use tauri::tray::TrayIconEvent;

            let open_item = MenuItemBuilder::with_id("open", "Ouvrir Kronn").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "Quitter").build(app)?;
            let tray_menu = MenuBuilder::new(app)
                .item(&open_item)
                .separator()
                .item(&quit_item)
                .build()?;

            if let Some(tray) = app.tray_by_id("main") {
                tray.set_menu(Some(tray_menu))?;

                // Handle tray menu clicks
                tray.on_menu_event(move |app, event| {
                    match event.id().as_ref() {
                        "open" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.unminimize();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => {
                            wake_lock::release();
                            app.exit(0);
                        }
                        _ => {}
                    }
                });

                // Double-click tray icon → show window
                tray.on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::DoubleClick { .. } = event {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                });
            }

            Ok(())
        })
        // Intercept window close → hide to tray instead of quitting
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent the window from actually closing — just hide it
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("Error while running Kronn Desktop");
}
