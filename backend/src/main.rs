use std::sync::Arc;
use tokio::sync::RwLock;

use kronn::{
    build_router,
    core::{config, mcp_scanner},
    db::Database,
    workflows::WorkflowEngine,
    AppState, DEFAULT_MAX_CONCURRENT_AGENTS,
};

// ─── Main ───────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config FIRST (before tracing init) so `debug_mode` can influence
    // the tracing filter's default level. This is a tiny re-order vs. the
    // historical flow — `config::load()` doesn't emit logs itself, so we
    // can afford to run it silently.
    let mut app_config = match config::load().await? {
        Some(cfg) => cfg,
        None => config::default_config(),
    };

    // 0.8.7 anti-hallucination — arm the process-global mode flag from config
    // so the runner chokepoint can gate P1/P2 without threading config through
    // every agent-spawn site. Re-set on every save (see api::setup).
    kronn::core::anti_halluc::set_mode(&app_config.server.anti_hallucination_mode);

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
    let filter_src = std::env::var("RUST_LOG")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| default_filter.to_string());
    use tracing_subscriber::prelude::*;
    let env_filter = tracing_subscriber::EnvFilter::new(&filter_src);
    // Persistent log file in the data dir (append, no ANSI). Without it a
    // crash or watcher-kill is only diagnosable from the operator's
    // terminal scrollback. Best-effort: an unwritable data dir must not
    // block boot.
    let file_layer = kronn::core::config::config_dir()
        .ok()
        .and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("kronn.log"))
                .ok()
        })
        .map(|file| {
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Arc::new(file))
        });
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(file_layer)
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
    // Host binding priority:
    //   1. `KRONN_HOST` env — explicit opt-in (e.g. `KRONN_HOST=0.0.0.0` to
    //      expose a NATIVE instance on the LAN for the contacts / P2P feature,
    //      which otherwise binds 127.0.0.1 and is unreachable cross-machine).
    //   2. Docker (`KRONN_DATA_DIR` set) → 0.0.0.0 so nginx can reach us.
    //   3. `config.server.host` (default 127.0.0.1 — localhost only).
    let host = match std::env::var("KRONN_HOST") {
        Ok(h) if !h.trim().is_empty() => h.trim().to_string(),
        _ if std::env::var("KRONN_DATA_DIR").is_ok() => "0.0.0.0".to_string(),
        _ => app_config.server.host.clone(),
    };
    // Record what we actually bound so the "Allow connections from other
    // devices" toggle can tell the UI whether a restart is still pending.
    kronn::core::net_expose::record_bound_host(&host);

    // Make the backend URL available to every child process we spawn —
    // the kronn-internal MCP bridge running inside the agent's child
    // process inherits this env var and calls back to the right port.
    // Pre-fix the script defaulted to :3140, which broke whenever the
    // backend ran on any other port (sandbox tests, custom configs).
    // We only set it when the operator hasn't already pinned a value
    // (Docker compose may inject the cluster-internal hostname).
    if std::env::var("KRONN_BACKEND_URL").is_err() {
        // Loopback is correct for both native and Docker: agents run
        // inside the same container/process tree as the backend, so
        // 127.0.0.1:<port> always reaches us. Nginx + cross-container
        // setups override this via the env.
        std::env::set_var("KRONN_BACKEND_URL", format!("http://127.0.0.1:{}", port));
    }

    // 0.8.11 — unify the API auth token across config.toml and the
    // KRONN_AUTH_TOKEN env, in BOTH directions, so every layer agrees:
    //   • env → config: an operator who sets KRONN_AUTH_TOKEN (e.g. in
    //     docker-compose) enables auth without editing config.toml — the auth
    //     middleware reads `config.server.auth_token`, so mirror the env into it.
    //   • config → env: spawned children (the `kronn-internal` MCP sidecar, a
    //     grandchild via the agent CLI) read KRONN_AUTH_TOKEN and send
    //     `Authorization: Bearer` — without it an auth-enabled / LAN-exposed
    //     instance returned a silent 401 to its own sidecar.
    let env_token = std::env::var("KRONN_AUTH_TOKEN")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    if let Some(ref t) = env_token {
        match &app_config.server.auth_token {
            None => app_config.server.auth_token = Some(t.clone()),
            // Both set but DIFFERENT: the middleware validates the config
            // token while spawned children (sidecar) inherit the env one —
            // every sidecar call 401s with no visible cause. Don't silently
            // pick a winner; tell the operator which one wins.
            Some(cfg) if cfg != t => tracing::warn!(
                "KRONN_AUTH_TOKEN env differs from the token in config.toml — the API \
                 validates the CONFIG token, but child processes (MCP sidecar, agent \
                 CLIs) inherit the ENV one and will get 401s. Align them (unset the env \
                 var, or clear server.auth_token in config.toml)."
            ),
            Some(_) => {}
        }
        // An operator who explicitly sets KRONN_AUTH_TOKEN is asking for auth
        // — and the LAN boot guard's own error message promises this env var
        // secures the instance. A token with auth_enabled=false is a no-op in
        // the middleware, so honor the intent.
        if !app_config.server.auth_enabled {
            tracing::info!("KRONN_AUTH_TOKEN set — enabling API authentication (was disabled)");
            app_config.server.auth_enabled = true;
        }
    }
    if let Some(ref token) = app_config.server.auth_token {
        if std::env::var("KRONN_AUTH_TOKEN").is_err() {
            std::env::set_var("KRONN_AUTH_TOKEN", token);
        }
    }
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

    // Security guard (0.8.11) — refuse to start when the operator exposed the
    // instance to the LAN but left the API unauthenticated. Secure-by-default:
    // a fresh `docker compose up` publishes on 127.0.0.1 (see docker-compose.yml
    // `KRONN_BIND`), so this never fires for the standard install. Exposing to
    // the LAN then requires a token, auth, or an explicit risk acknowledgment.
    // Real container detection (KRONN_IN_DOCKER / /.dockerenv) — NOT the old
    // KRONN_DATA_DIR proxy: a native install relocating its data dir must not
    // silently switch the guard to docker mode (where an unset KRONN_BIND
    // reads as "not exposed" even when the actual bind host is 0.0.0.0).
    let is_docker = kronn::core::env::is_docker();
    let kronn_bind = std::env::var("KRONN_BIND").ok();
    let ack_insecure = std::env::var("KRONN_ALLOW_INSECURE_LAN")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let exposed = kronn::core::net_expose::lan_exposed(is_docker, kronn_bind.as_deref(), &host);
    if let Some(msg) = kronn::core::net_expose::insecure_lan_boot_error(
        exposed,
        app_config.server.auth_enabled,
        app_config.server.auth_token.is_some(),
        ack_insecure,
    ) {
        tracing::error!("{msg}");
        return Err(anyhow::anyhow!(msg));
    }

    // Exactly ONE backend per data dir. Refuse to start if another instance
    // already holds the lock — prevents two processes (a stale one, or P2P peers
    // sharing a synced dir) racing on config.toml / the key / the DB. Held for
    // the whole process lifetime; released when `_data_dir_lock` drops at exit.
    let _data_dir_lock = config::acquire_data_dir_lock().map_err(|e| {
        tracing::error!("{e}");
        e
    })?;

    // Open database
    let database = Arc::new(Database::open().expect("Failed to open database"));
    tracing::info!(
        "Database opened at {}/kronn.db",
        config::config_dir().unwrap().display()
    );

    // Resolve/repair the encryption key now that the DB is open — `config::load`
    // deliberately never mints one. This adopts the legacy config.toml key,
    // restores it from the keychain/sidecar, or mints on a genuinely empty
    // install, and NEVER regenerates a key over existing encrypted data (the
    // silent regen that orphaned every secret on 2026-06-30). Fail-soft: an
    // unresolvable key locks only the token subsystem, it never blocks boot.
    match kronn::core::keystore::reconcile(&mut app_config, &database).await {
        Ok(outcome) => tracing::info!("Encryption key reconciled: {outcome:?}"),
        Err(e) => tracing::error!("Key reconcile failed (booting locked): {e}"),
    }

    // Build state via the shared factory — keep both mains in sync when
    // new runtime fields are added to AppState (see lib.rs doc).
    let config_arc = Arc::new(RwLock::new(app_config));
    let state = AppState::new_defaults(config_arc, database, max_agents);

    // Fire up the kronn-docs sidecar in the background — its start is
    // best-effort (graceful skip if deps are missing) so we don't block
    // the backend boot on it.
    {
        let sc = state.docs_sidecar.clone();
        tokio::spawn(async move { sc.start().await });
    }

    // B6 — webhook the runs the PREVIOUS process died holding (flipped to
    // Interrupted by the boot reconcile). The engine-spawn tail can never
    // notify these; without this, "cron died at 6am" stays silent — the
    // exact case the failure webhook exists for. Best-effort, bounded.
    {
        let st = state.clone();
        tokio::spawn(async move {
            kronn::core::run_notify::notify_boot_interrupted(&st).await;
        });
    }

    // Workflow engine gets a clone of the state so it can spawn runs that
    // need full access (batch fan-out, ws broadcasts, agent semaphore).
    let workflow_engine = Arc::new(WorkflowEngine::new(state.clone()));

    // ── Orphan scan (workflow_runs) ─────────────────────────────────────────
    // Superseded by the boot reconcile in `Database::open_path` (0.8.11 B5):
    // zombies are flipped to `Interrupted` there — the terminal state the UI,
    // the duration averages, and the failure webhook all expect. The legacy
    // UPDATE here marked them `Failed` AFTER that reconcile (so it always
    // matched 0 rows) and would have mislabeled + bypassed the boot
    // notification if it ever fired. Kept as an assertion-style probe only.
    match state
        .db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM workflow_runs WHERE status IN ('Running', 'Pending')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
        .await
    {
        Ok(0) => tracing::info!("Orphan scan: nothing to clean up (boot reconcile already ran)"),
        Ok(n) => tracing::warn!(
            target: "kronn::invariant",
            "Orphan scan: {n} workflow_runs still Running/Pending AFTER the boot reconcile — \
             this should be impossible, investigate db::open_path ordering"
        ),
        Err(e) => tracing::warn!("Orphan scan probe failed: {}", e),
    }

    // ── Registry sync ─────────────────────────────────────────────────────
    // Re-mirror registry-managed fields (`api_spec`, `description`,
    // `transport`) onto existing DB rows so users who configured a plugin
    // before a registry enrichment (e.g. GitHub gained `api_spec` after
    // its initial config was saved) don't have to click "Refresh registry"
    // by hand for the workflow wizard to see the new capability.
    let synced = state
        .db
        .with_conn(|conn| {
            let registry = kronn::core::registry::builtin_registry();
            kronn::db::mcps::sync_registry_servers_to_db(conn, &registry)
        })
        .await;
    match synced {
        Ok(n) if n > 0 => tracing::info!(
            "Registry sync: refreshed {} existing MCP server row(s) from builtin registry",
            n
        ),
        Ok(_) => tracing::info!("Registry sync: nothing to refresh (no registered plugins yet)"),
        Err(e) => tracing::warn!("Registry sync failed: {}", e),
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
    let recovered = state
        .db
        .with_conn(kronn::db::discussions::recover_partial_responses)
        .await;
    match recovered {
        Ok(ids) if !ids.is_empty() => {
            tracing::warn!(
                "Partial-response recovery: {} discussion(s) had in-flight agent output \
                 from a previous process, saved as Agent messages with footer",
                ids.len()
            );
            let _ = state
                .ws_broadcast
                .send(kronn::models::WsMessage::PartialResponseRecovered {
                    discussion_ids: ids,
                });
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Partial-response recovery failed: {}", e),
    }

    // Boot reconcile n°3, AFTER partial recovery. Catches the work
    // the other two reconciles miss: an agent run that was OWED (a queued batch
    // child, or a human message whose auto-reply never spawned) and was
    // orphaned by the restart before producing any durable trace. It marks
    // them interrupted (a notice message) — it NEVER re-spawns, because an
    // interruption may be deliberate (the user shut the machine). Runs after
    // recovery so a disc whose partial was just turned into an Agent message is
    // correctly seen as answered and skipped.
    let awaiting = state
        .db
        .with_conn(kronn::db::discussions::reconcile_awaiting_agents)
        .await;
    match awaiting {
        Ok(ids) if !ids.is_empty() => {
            tracing::warn!(
                "Awaiting-agent reconcile: {} discussion(s) were owed an agent run that never \
                 started before the restart — marked interrupted (not re-spawned)",
                ids.len()
            );
            let _ = state
                .ws_broadcast
                .send(kronn::models::WsMessage::AgentRunsInterrupted {
                    discussion_ids: ids,
                });
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Awaiting-agent reconcile failed: {}", e),
    }

    // 0.8.11 (B7) — opt-in run retention. Only when the operator set
    // `run_retention_days > 0`; parent runs referenced by a retained child are
    // preserved by the query. Default 0 = keep all history (no surprise loss).
    let retention_days = state.config.read().await.server.run_retention_days;
    if retention_days > 0 {
        match state
            .db
            .with_conn(move |conn| {
                kronn::db::workflows::purge_runs_older_than(conn, retention_days)
            })
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                "Run retention: purged {n} workflow run(s) older than {retention_days} days",
            ),
            Err(e) => tracing::warn!("Run retention purge failed: {}", e),
        }
    }

    // Reap abandoned MCP sessions (2026-06-08). `count_live_participants` is
    // presence-sticky — any `status='active'` session suppresses Kronn's
    // auto-response (no per-message staleness window, which had wrongly
    // judged idle turn-based peers as dead → double-responder). To keep
    // 'active' honest, retire sessions idle > 24h (agents that exited
    // without `disc_leave`) at every boot. Migration 065 does the same once;
    // this keeps it self-maintaining across restarts.
    match state
        .db
        .with_conn(kronn::db::discussion_sessions::reap_abandoned_sessions)
        .await
    {
        Ok(n) if n > 0 => {
            tracing::info!("Reaped {n} abandoned discussion session(s) (idle > 24h, no disc_leave)")
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Abandoned-session reap failed: {}", e),
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
            match config::save(&config).await {
                Ok(_) => tracing::info!("Auto-imported {} API key(s) from agent configs", imported),
                // Don't log success over a failed persist: the keys exist only
                // in memory and silently vanish (with any user edits layered
                // on them) at the next restart.
                Err(e) => tracing::error!(
                    "Auto-imported {} API key(s) but saving config.toml FAILED: {e} — keys are in-memory only until the next successful save",
                    imported
                ),
            }
        }
    }

    // Sync all MCP configs to disk on startup (ensures all agents have up-to-date configs)
    {
        let db = state.db.clone();
        let cfg = state.config.read().await;
        if let Some(secret) = cfg.encryption_secret.clone() {
            drop(cfg); // Release read lock before blocking
            if let Err(e) = db
                .with_conn(move |conn| {
                    mcp_scanner::sync_all_projects(conn, &secret);
                    Ok(())
                })
                .await
            {
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
        if let Err(e) = db
            .with_conn(|conn| {
                let projects = kronn::db::projects::list_projects(conn)?;
                for p in &projects {
                    let resolved = kronn::core::scanner::resolve_host_path(&p.path);
                    if resolved.join(".kronn").exists()
                        || resolved.join(".kronn-tmp").exists()
                        || resolved.join(".kronn-worktrees").exists()
                    {
                        mcp_scanner::ensure_gitignore_public(&p.path, ".kronn/");
                    }
                }
                Ok(())
            })
            .await
        {
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

    // 0.9.0 — Continual Learning staleness sweep (hourly). Spawn mirrored in
    // desktop/src-tauri/src/main.rs — the feature is in the lib, the spawn is
    // per-binary.
    let learning_sweep = std::sync::Arc::new(kronn::core::learning_sweep::LearningSweep::new(
        state.db.clone(),
    ));
    tokio::spawn(async move { learning_sweep.start().await });

    // 0.8.11 (B6) — periodic DB backup (default every 24h, keep 7). Targets
    // KRONN_BACKUP_DIR (a host-mounted dir outside the data volume) when set;
    // KRONN_BACKUP_INTERVAL_HOURS=0 disables it entirely.
    if let Some(backup_scheduler) = kronn::core::backup::BackupScheduler::from_env(state.db.clone())
    {
        tokio::spawn(async move { backup_scheduler.start().await });
    }

    // Start WebSocket client manager (outbound connections to contacts)
    let ws_state = state.clone();
    tokio::spawn(async move { kronn::core::ws_client::run(ws_state).await });

    // Build router
    let app = build_router(state);

    // Start server
    let addr = format!("{}:{}", host, port);
    tracing::info!("Listening on {}", addr);

    // Banner entry URL. `kronn start-dev` exports KRONN_DEV_UI_URL (the Vite
    // dev UI): in native dev THIS port serves the API only, so the banner must
    // point at the UI. Without the override — Docker (the gateway serves the UI
    // on this port) or a bare backend — it shows the backend address as before.
    let backend_url = format!("http://{}:{}", host, port);
    let dev_ui = std::env::var("KRONN_DEV_UI_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let entry = banner_entry_url(&backend_url, dev_ui.as_deref());

    println!();
    println!("  ╔═══════════════════════════════════════╗");
    println!("  ║                                       ║");
    println!("  ║   K R O N N   v{:<23}║", env!("CARGO_PKG_VERSION"));
    println!("  ║   ─────────────────                   ║");
    println!("  ║   Entering the grid...                ║");
    println!("  ║                                       ║");
    println!("  ║   → {:<32}║", entry);
    println!("  ║   Agents: max {} concurrent          ║", max_agents);
    println!("  ║                                       ║");
    println!("  ╚═══════════════════════════════════════╝");
    println!();
    if let Some(ref ui) = dev_ui {
        println!("  Native dev — open the UI at {}", osc8_link(ui));
        println!("  ({} is the API only, not the UI.)", backend_url);
        println!();
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // Graceful shutdown: wait for SIGTERM/SIGINT, then let in-flight requests finish
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!("Kronn — Shutdown complete.");
    Ok(())
}

/// URL shown in the startup banner's "→" line. Returns the dev-UI override when
/// it is set and non-empty (`kronn start-dev` exports KRONN_DEV_UI_URL — in
/// native dev the listen port serves the API only), otherwise the backend
/// address (Docker, where the gateway serves the UI on this port, or a bare
/// backend with no separate UI server).
fn banner_entry_url(backend_url: &str, dev_ui: Option<&str>) -> String {
    match dev_ui {
        Some(ui) if !ui.is_empty() => ui.to_string(),
        _ => backend_url.to_string(),
    }
}

/// Wrap `url` in an OSC 8 terminal hyperlink so it's clickable (iTerm2,
/// Terminal.app, WezTerm, kitty, VS Code…) — but only when stdout is a real
/// terminal. Piped/redirected output (logs, CI) gets the plain URL so no escape
/// bytes leak into files.
fn osc8_link(url: &str) -> String {
    use std::io::IsTerminal;
    osc8(url, std::io::stdout().is_terminal())
}

/// Pure core of [`osc8_link`] — split out so the escape construction is unit
/// testable without a TTY. `tty=false` returns the bare URL.
fn osc8(url: &str, tty: bool) -> String {
    if tty {
        // ESC ] 8 ; ; <url> ST <url> ESC ] 8 ; ; ST   (ST = ESC \)
        format!("\x1b]8;;{url}\x1b\\{url}\x1b]8;;\x1b\\")
    } else {
        url.to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_uses_backend_url_without_override() {
        assert_eq!(
            banner_entry_url("http://127.0.0.1:3140", None),
            "http://127.0.0.1:3140",
            "no override (Docker / bare backend) → show the backend address"
        );
    }

    #[test]
    fn banner_uses_dev_ui_override_when_set() {
        // Native dev: :3140 is API-only, so the banner must point at the Vite UI.
        assert_eq!(
            banner_entry_url("http://127.0.0.1:3140", Some("http://localhost:5173")),
            "http://localhost:5173"
        );
    }

    #[test]
    fn banner_ignores_empty_override() {
        assert_eq!(
            banner_entry_url("http://127.0.0.1:3140", Some("")),
            "http://127.0.0.1:3140",
            "an empty KRONN_DEV_UI_URL must not blank out the banner"
        );
    }

    #[test]
    fn osc8_plain_when_not_a_tty() {
        // Redirected/piped output (logs, CI) must stay free of escape bytes —
        // cross-platform, no OS gating.
        assert_eq!(
            osc8("http://localhost:5173", false),
            "http://localhost:5173"
        );
    }

    #[test]
    fn osc8_wraps_url_in_hyperlink_on_a_tty() {
        let s = osc8("http://localhost:5173", true);
        assert!(
            s.starts_with("\x1b]8;;http://localhost:5173\x1b\\"),
            "opens the OSC 8 link: {s:?}"
        );
        assert!(
            s.ends_with("\x1b]8;;\x1b\\"),
            "closes the OSC 8 link: {s:?}"
        );
        assert!(
            s.contains("http://localhost:5173"),
            "keeps the visible URL label"
        );
    }
}
