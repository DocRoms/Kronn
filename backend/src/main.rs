use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};

use kronn::{build_router, core::{config, mcp_scanner}, db::Database, workflows::WorkflowEngine, AppState, AuditTracker, DEFAULT_MAX_CONCURRENT_AGENTS};

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config FIRST (before tracing init) so `debug_mode` can influence
    // the tracing filter's default level. This is a tiny re-order vs. the
    // historical flow — `config::load()` doesn't emit logs itself, so we
    // can afford to run it silently.
    let app_config = match config::load().await? {
        Some(cfg) => cfg,
        None => config::default_config(),
    };

    // Initialize tracing — write to stdout (Docker best practice: stdout for
    // logs, stderr for panics) AND to the in-memory ringbuffer that the
    // Settings > Debug viewer reads via `GET /api/debug/logs`.
    //
    // Filter precedence:
    //   1. `RUST_LOG` env var if set AND non-empty (CLI `--debug` flag /
    //      `make start DEBUG=1` sets `RUST_LOG=kronn=debug,tower_http=debug`
    //      — always wins).
    //   2. `config.server.debug_mode = true` → default to `debug`.
    //   3. Default `info` (production-friendly).
    //
    // Bug 2026-04-15: docker-compose writes `RUST_LOG=${KRONN_RUST_LOG:-}`
    // which resolves to an EMPTY STRING (not unset) when `KRONN_RUST_LOG`
    // isn't defined. `try_from_default_env()` doesn't treat empty as
    // missing — it parses `""` into a filter that matches nothing, so
    // the debug_mode toggle and the log viewer silently died together.
    // Fix: treat whitespace-only `RUST_LOG` as "not set".
    let default_filter = if app_config.server.debug_mode {
        "kronn=debug,tower_http=debug"
    } else {
        "kronn=info,tower_http=info"
    };
    let filter_src = std::env::var("RUST_LOG").ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_filter.to_string());
    use tracing_subscriber::prelude::*;
    let env_filter = tracing_subscriber::EnvFilter::new(&filter_src);
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(kronn::core::log_buffer::BufferLayer)
        .init();

    tracing::info!("tracing initialized — filter: {}", filter_src);

    tracing::info!("Kronn — Entering the grid...");
    if app_config.server.debug_mode {
        tracing::info!(
            "Debug mode is ON (config.server.debug_mode = true). To turn off: Settings UI or edit config.toml."
        );
    }
    let config_source = config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".into());
    tracing::info!("Config loaded from {}", config_source);

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

    // Build state first (no longer holds workflow_engine — broken circular dep)
    let config_arc = Arc::new(RwLock::new(app_config));
    let (ws_tx, _) = tokio::sync::broadcast::channel::<kronn::models::WsMessage>(256);
    let state = AppState {
        config: config_arc,
        db: database,
        agent_semaphore: Arc::new(Semaphore::new(max_agents)),
        audit_tracker: Arc::new(std::sync::Mutex::new(AuditTracker::default())),
        ws_broadcast: Arc::new(ws_tx),
        cancel_registry: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
    };

    // Workflow engine gets a clone of the state so it can spawn runs that
    // need full access (batch fan-out, ws broadcasts, agent semaphore).
    let workflow_engine = Arc::new(WorkflowEngine::new(state.clone()));

    // ── Orphan scan ────────────────────────────────────────────────────────
    // Previous process may have crashed/been killed while workflow_runs or
    // discussions were in the "Running" state. Nothing is listening for them
    // anymore (cancel registry is empty at boot), so the UI would show them
    // as running forever without this cleanup. We mark any still-Running row
    // as Failed with a clear note.
    let cleaned = state.db.with_conn(|conn| {
        let runs = conn.execute(
            "UPDATE workflow_runs SET status = 'Failed', finished_at = datetime('now') \
             WHERE status = 'Running'",
            [],
        )?;
        // Append a marker message to the last agent response so the UI shows
        // what happened. We can't easily UPDATE the last message content from
        // here without rehydrating the messages table schema, so we just log
        // the count — the disc will show as "no reply yet" until re-run.
        Ok(runs)
    }).await;
    match cleaned {
        Ok(n) if n > 0 => tracing::warn!(
            "Orphan scan: {} workflow_runs left Running by previous process, marked as Failed", n
        ),
        Ok(_) => tracing::info!("Orphan scan: nothing to clean up"),
        Err(e) => tracing::warn!("Orphan scan failed: {}", e),
    }

    // Partial-response recovery — agents whose `full_response` was being
    // checkpointed into discussions.partial_response when the previous
    // process died. Convert each into an Agent message with an "interrupted"
    // footer so the user sees what was thought instead of a silent gap.
    //
    // Broadcast PartialResponseRecovered over the WS so any already-connected
    // frontend refetches the affected discs and shows the recovered messages
    // immediately — without this, a user who reopens the app before the
    // recovery finishes and retypes their prompt ends up with two agent
    // responses on the same disc (the recovered one + the new run).
    let recovered = state.db.with_conn(|conn| {
        kronn::db::discussions::recover_partial_responses(conn)
    }).await;
    match recovered {
        Ok(ids) if !ids.is_empty() => {
            tracing::warn!(
                "Partial-response recovery: {} discussion(s) had in-flight agent output \
                 from a previous process, saved as Agent messages with footer",
                ids.len()
            );
            let _ = state.ws_broadcast.send(
                kronn::models::WsMessage::PartialResponseRecovered {
                    discussion_ids: ids,
                }
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Partial-response recovery failed: {}", e),
    }

    // Auto-discover and import API keys from agent config files (~/.vibe/.env, ~/.codex/auth.json, etc.)
    {
        let discovered = kronn::core::key_discovery::discover_keys().await;
        let mut config = state.config.write().await;
        let mut imported = 0u32;
        for dk in discovered {
            if !config.tokens.keys.iter().any(|k| k.value == dk.value) {
                let is_first = !config.tokens.keys.iter().any(|k| k.provider == dk.provider);
                config.tokens.keys.push(kronn::models::ApiKey {
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
            let _ = config::save(&config).await;
            tracing::info!("Auto-imported {} API key(s) from agent configs", imported);
        }
    }

    // Sync all MCP configs to disk on startup (ensures all agents have up-to-date configs)
    {
        let db = state.db.clone();
        let cfg = state.config.read().await;
        if let Some(secret) = cfg.encryption_secret.clone() {
            drop(cfg); // Release read lock before blocking
            if let Err(e) = db.with_conn(move |conn| {
                mcp_scanner::sync_all_projects(conn, &secret);
                Ok(())
            }).await {
                tracing::warn!("MCP startup sync failed: {}", e);
            } else {
                tracing::info!("MCP configs synced to disk for all projects");
            }
        } else {
            tracing::debug!("No encryption secret — skipping MCP startup sync");
        }
    }

    // Ensure .kronn/ is gitignored in all projects
    // (retroactive fix — also migrates old .kronn-tmp/ and .kronn-worktrees/ patterns)
    {
        let db = state.db.clone();
        if let Err(e) = db.with_conn(|conn| {
            let projects = kronn::db::projects::list_projects(conn)?;
            for p in &projects {
                let resolved = kronn::core::scanner::resolve_host_path(&p.path);
                if resolved.join(".kronn").exists() || resolved.join(".kronn-tmp").exists() || resolved.join(".kronn-worktrees").exists() {
                    mcp_scanner::ensure_gitignore_public(&p.path, ".kronn/");
                }
            }
            Ok(())
        }).await {
            tracing::warn!("Gitignore startup fix failed: {}", e);
        }
    }

    // Migrate worktrees from /data/workspaces/ to .kronn/worktrees/ inside each repo
    {
        let db = state.db.clone();
        if let Err(e) = db.with_conn(|conn| {
            let projects = kronn::db::projects::list_projects(conn)?;
            let discussions = kronn::db::discussions::list_discussions(conn)?;

            for disc in &discussions {
                let old_path = match &disc.workspace_path {
                    Some(p) if p.starts_with("/data/workspaces/") => p.clone(),
                    _ => continue,
                };
                let branch = match &disc.worktree_branch {
                    Some(b) => b.clone(),
                    None => continue,
                };
                let project = match projects.iter().find(|p| Some(&p.id) == disc.project_id.as_ref()) {
                    Some(p) => p,
                    None => continue,
                };

                let resolved = kronn::core::scanner::resolve_host_path(&project.path);
                let repo_path = std::path::Path::new(&resolved);

                match kronn::core::worktree::reattach_worktree(
                    repo_path, &project.name, &disc.title, &branch,
                ) {
                    Ok(info) => {
                        let _ = kronn::db::discussions::update_discussion_workspace(
                            conn, &disc.id, &info.path, &info.branch,
                        );
                        tracing::info!(
                            "Migrated worktree for discussion '{}': {} -> {}",
                            disc.title, old_path, info.path
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to migrate worktree for '{}': {} — clearing stale path",
                            disc.title, e
                        );
                        // Clear stale path so it doesn't keep failing
                        let _ = conn.execute(
                            "UPDATE discussions SET workspace_path = NULL, worktree_branch = NULL WHERE id = ?1",
                            rusqlite::params![disc.id],
                        );
                    }
                }
            }
            Ok(())
        }).await {
            tracing::warn!("Worktree migration failed: {}", e);
        }
    }

    // Start workflow engine in background
    let engine = workflow_engine.clone();
    tokio::spawn(async move { engine.start().await });

    // Start WebSocket client manager (outbound connections to contacts)
    let ws_state = state.clone();
    tokio::spawn(async move { kronn::core::ws_client::run(ws_state).await });

    // Build router
    let app = build_router(state);

    // Start server
    let addr = format!("{}:{}", host, port);
    tracing::info!("Listening on {}", addr);
    println!();
    println!("  ╔═══════════════════════════════════════╗");
    println!("  ║                                       ║");
    println!("  ║   K R O N N   v{:<23}║", env!("CARGO_PKG_VERSION"));
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
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
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
