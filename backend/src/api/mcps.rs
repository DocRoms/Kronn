use axum::{extract::{Path, Query, State}, Json};
use rusqlite::{params, Connection};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::*;
use crate::core::{registry, mcp_scanner};
use crate::db;
use crate::AppState;

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
}

/// GET /api/mcps/registry?q=search
pub async fn list_registry(
    Query(params): Query<SearchQuery>,
) -> Json<ApiResponse<Vec<McpDefinition>>> {
    let results = match params.q {
        Some(q) if !q.is_empty() => registry::search(&q),
        _ => registry::builtin_registry(),
    };
    Json(ApiResponse::ok(results))
}

/// GET /api/mcps — full overview: servers + configs (masked)
pub async fn overview(
    State(state): State<AppState>,
) -> Json<ApiResponse<McpOverview>> {
    match state.db.with_conn(|conn| {
        let servers = db::mcps::list_servers(conn)?;
        let configs = db::mcps::list_configs_display(conn)?;
        let projects = db::projects::list_projects(conn)?;
        let customized_contexts = build_customized_contexts(&configs, &projects);

        Ok(McpOverview { servers, configs, customized_contexts })
    }).await {
        Ok(data) => Json(ApiResponse::ok(data)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/mcps/configs — create a new MCP config
/// server_id can be an existing DB server ID or a registry ID (auto-creates server)
pub async fn create_config(
    State(state): State<AppState>,
    Json(req): Json<CreateMcpConfigRequest>,
) -> Json<ApiResponse<McpConfigDisplay>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    let reg = registry::builtin_registry();

    let result = state.db.with_conn(move |conn| {
        // Find server in DB, or create from registry
        let servers = db::mcps::list_servers(conn)?;
        let server = if let Some(s) = servers.iter().find(|s| s.id == req.server_id) {
            s.clone()
        } else if let Some(def) = reg.iter().find(|d| d.id == req.server_id) {
            // Auto-create server from registry
            let s = McpServer {
                id: def.id.clone(),
                name: def.name.clone(),
                description: def.description.clone(),
                transport: def.transport.clone(),
                source: McpSource::Registry,
            };
            db::mcps::upsert_server(conn, &s)?;
            s
        } else {
            return Err(anyhow::anyhow!("Server '{}' not found in DB or registry", req.server_id));
        };

        // Compute config hash for dedup
        let hash = db::mcps::compute_config_hash(&server, &req.env, req.args_override.as_ref());

        // Check if identical config exists
        if let Some(existing) = db::mcps::find_config_by_hash(conn, &hash)? {
            // Merge project_ids
            let mut all_pids: Vec<String> = existing.project_ids.clone();
            for pid in &req.project_ids {
                if !all_pids.contains(pid) {
                    all_pids.push(pid.clone());
                }
            }
            db::mcps::set_config_projects(conn, &existing.id, &all_pids)?;

            // Return updated display
            let configs = db::mcps::list_configs_display(conn)?;
            let display = configs.into_iter().find(|c| c.id == existing.id)
                .ok_or_else(|| anyhow::anyhow!("Config disappeared"))?;
            return Ok(display);
        }

        // Encrypt env
        let env_encrypted = db::mcps::encrypt_env(&req.env, &secret)
            .map_err(|e| anyhow::anyhow!("Encryption error: {}", e))?;

        let env_keys: Vec<String> = req.env.keys().cloned().collect();

        let config = McpConfig {
            id: Uuid::new_v4().to_string(),
            server_id: req.server_id,
            label: req.label,
            env_keys,
            env_encrypted,
            args_override: req.args_override,
            is_global: req.is_global,
            config_hash: hash,
            project_ids: req.project_ids,
        };

        db::mcps::insert_config(conn, &config)?;

        // Sync .mcp.json to disk for affected projects
        let mut sync_pids = config.project_ids.clone();
        if config.is_global {
            // Global config affects all projects
            if let Ok(projects) = db::projects::list_projects(conn) {
                sync_pids = projects.iter().map(|p| p.id.clone()).collect();
            }
        }
        mcp_scanner::sync_affected_projects(conn, &sync_pids, &secret);

        let configs = db::mcps::list_configs_display(conn)?;
        let display = configs.into_iter().find(|c| c.id == config.id)
            .ok_or_else(|| anyhow::anyhow!("Config disappeared after insert"))?;
        Ok(display)
    }).await;

    match result {
        Ok(display) => Json(ApiResponse::ok(display)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// PUT /api/mcps/configs/:id — update config
pub async fn update_config(
    State(state): State<AppState>,
    Path(config_id): Path<String>,
    Json(req): Json<UpdateMcpConfigRequest>,
) -> Json<ApiResponse<McpConfigDisplay>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    let result = state.db.with_conn(move |conn| {
        // Get config before update to know old state
        let old_config = db::mcps::get_config(conn, &config_id)?
            .ok_or_else(|| anyhow::anyhow!("Config not found"))?;

        let (env_encrypted, env_keys, new_hash) = if let Some(ref env) = req.env {
            let encrypted = db::mcps::encrypt_env(env, &secret)
                .map_err(|e| anyhow::anyhow!("Encryption error: {}", e))?;
            let keys: Vec<String> = env.keys().cloned().collect();

            // Recompute hash
            let servers = db::mcps::list_servers(conn)?;
            let server = servers.iter().find(|s| s.id == old_config.server_id)
                .ok_or_else(|| anyhow::anyhow!("Server not found"))?;
            let hash = db::mcps::compute_config_hash(
                server,
                env,
                req.args_override.as_ref().or(old_config.args_override.as_ref()),
            );

            (Some(encrypted), Some(keys), Some(hash))
        } else {
            (None, None, None)
        };

        db::mcps::update_config(
            conn,
            &config_id,
            req.label.as_deref(),
            env_encrypted.as_deref(),
            env_keys.as_deref(),
            req.args_override.as_ref(),
            req.is_global,
            new_hash.as_deref(),
        )?;

        // Sync .mcp.json to disk
        let global_changed = req.is_global.map(|g| g != old_config.is_global).unwrap_or(false);
        let new_global = req.is_global.unwrap_or(old_config.is_global);
        if global_changed || new_global {
            // Global flag changed or is active → sync all projects
            mcp_scanner::sync_all_projects(conn, &secret);
        } else {
            mcp_scanner::sync_affected_projects(conn, &old_config.project_ids, &secret);
        }

        let configs = db::mcps::list_configs_display(conn)?;
        configs.into_iter().find(|c| c.id == config_id)
            .ok_or_else(|| anyhow::anyhow!("Config not found after update"))
    }).await;

    match result {
        Ok(display) => Json(ApiResponse::ok(display)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// DELETE /api/mcps/configs/:id
pub async fn delete_config(
    State(state): State<AppState>,
    Path(config_id): Path<String>,
) -> Json<ApiResponse<()>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    match state.db.with_conn(move |conn| {
        // Get config before deleting to know affected projects
        let config = db::mcps::get_config(conn, &config_id)?;
        let result = db::mcps::delete_config(conn, &config_id)?;

        // Sync affected projects
        if let Some(cfg) = config {
            if cfg.is_global {
                mcp_scanner::sync_all_projects(conn, &secret);
            } else {
                mcp_scanner::sync_affected_projects(conn, &cfg.project_ids, &secret);
            }
        }

        Ok(result)
    }).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Config not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PUT /api/mcps/configs/:id/projects — set project linkages
pub async fn set_config_projects(
    State(state): State<AppState>,
    Path(config_id): Path<String>,
    Json(req): Json<LinkMcpConfigRequest>,
) -> Json<ApiResponse<()>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    match state.db.with_conn(move |conn| {
        // Get old project_ids before update
        let old_config = db::mcps::get_config(conn, &config_id)?;
        let old_pids = old_config.map(|c| c.project_ids).unwrap_or_default();

        db::mcps::set_config_projects(conn, &config_id, &req.project_ids)?;

        // Sync all affected projects (old ones that lost the config + new ones that got it)
        let mut all_pids: Vec<String> = old_pids;
        for pid in &req.project_ids {
            if !all_pids.contains(pid) {
                all_pids.push(pid.clone());
            }
        }
        mcp_scanner::sync_affected_projects(conn, &all_pids, &secret);

        Ok(())
    }).await {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/mcps/configs/:id/reveal — decrypt and reveal env secrets
pub async fn reveal_secrets(
    State(state): State<AppState>,
    Path(config_id): Path<String>,
) -> Json<ApiResponse<Vec<McpEnvEntry>>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    let result = state.db.with_conn(move |conn| {
        let config = db::mcps::get_config(conn, &config_id)?
            .ok_or_else(|| anyhow::anyhow!("Config not found"))?;

        let env = db::mcps::decrypt_env(&config.env_encrypted, &secret)
            .map_err(|e| anyhow::anyhow!("Decryption error: {}", e))?;

        let entries: Vec<McpEnvEntry> = env.into_iter()
            .map(|(k, v)| McpEnvEntry { key: k, masked_value: v })
            .collect();
        Ok(entries)
    }).await;

    match result {
        Ok(entries) => Json(ApiResponse::ok(entries)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// POST /api/mcps/refresh — scan all projects for MCP configs, upsert to new system
pub async fn refresh(
    State(state): State<AppState>,
) -> Json<ApiResponse<McpOverview>> {
    let config_read = state.config.read().await;
    let secret = match &config_read.encryption_secret {
        Some(s) => s.clone(),
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    drop(config_read);

    let reg = registry::builtin_registry();

    let result = state.db.with_conn(move |conn| {
        // Migrate old detected:* servers to registry IDs where possible
        migrate_detected_to_registry(conn, &reg)?;

        // Update registry servers' transport/description from current registry
        // (handles package renames, description changes, etc.)
        for def in &reg {
            let server = McpServer {
                id: def.id.clone(),
                name: def.name.clone(),
                description: def.description.clone(),
                transport: def.transport.clone(),
                source: McpSource::Registry,
            };
            // Only upsert if server already exists in DB
            let exists = db::mcps::list_servers(conn)?
                .iter().any(|s| s.id == def.id);
            if exists {
                db::mcps::upsert_server(conn, &server)?;
            }
        }

        // Rehash existing configs to match updated server transports
        // (prevents duplicates when registry transport changes slightly)
        rehash_configs(conn, &secret)?;

        let projects = db::projects::list_projects(conn)?;

        for project in &projects {
            let parsed = match mcp_scanner::read_mcp_json(&project.path) {
                Some(p) => p,
                None => continue,
            };

            for (name, entry) in &parsed.mcp_servers {
                // Determine transport
                let transport = if let Some(cmd) = &entry.command {
                    McpTransport::Stdio {
                        command: cmd.clone(),
                        args: entry.args.clone().unwrap_or_default(),
                    }
                } else if let Some(url) = &entry.url {
                    McpTransport::Sse { url: url.clone() }
                } else {
                    continue;
                };

                // Try to match against registry by command+args
                let registry_match = match_registry_entry(entry, &reg);

                let (server_id, server_name, description, source, server_transport) = if let Some(def) = registry_match {
                    (def.id.clone(), def.name.clone(), def.description.clone(), McpSource::Registry, def.transport.clone())
                } else {
                    let desc = if let Some(cmd) = &entry.command {
                        let args = entry.args.as_deref().unwrap_or(&[]);
                        let pkg = args.iter()
                            .find(|a| !a.starts_with('-'))
                            .map(|s| s.as_str())
                            .unwrap_or("");
                        format!("{} {}", cmd, pkg).trim().to_string()
                    } else if let Some(url) = &entry.url {
                        url.clone()
                    } else {
                        name.to_string()
                    };
                    (format!("detected:{}", name), name.clone(), desc, McpSource::Detected, transport.clone())
                };

                let server = McpServer {
                    id: server_id.clone(),
                    name: server_name,
                    description,
                    transport: server_transport,
                    source,
                };
                db::mcps::upsert_server(conn, &server)?;

                // Compute config hash
                let hash = db::mcps::compute_config_hash(&server, &entry.env, None);

                // Check if config with this hash already exists
                if let Some(existing) = db::mcps::find_config_by_hash(conn, &hash)? {
                    // Just link project if not already linked
                    if !existing.project_ids.contains(&project.id) {
                        db::mcps::link_config_project(conn, &existing.id, &project.id)?;
                    }
                } else {
                    // Create new config
                    let env_encrypted = db::mcps::encrypt_env(&entry.env, &secret)
                        .map_err(|e| anyhow::anyhow!("Encrypt error: {}", e))?;
                    let env_keys: Vec<String> = entry.env.keys().cloned().collect();

                    let config = McpConfig {
                        id: Uuid::new_v4().to_string(),
                        server_id: server_id.clone(),
                        label: name.clone(),
                        env_keys,
                        env_encrypted,
                        args_override: None,
                        is_global: false,
                        config_hash: hash,
                        project_ids: vec![project.id.clone()],
                    };
                    db::mcps::insert_config(conn, &config)?;
                }
            }
        }

        // Deduplicate configs with the same hash (merge project linkages, keep oldest)
        dedup_configs(conn)?;

        // Clean up orphan servers (no configs pointing to them)
        conn.execute_batch(
            "DELETE FROM mcp_servers WHERE id NOT IN (SELECT DISTINCT server_id FROM mcp_configs)"
        )?;

        // Sync all .mcp.json files to disk (picks up transport updates)
        mcp_scanner::sync_all_projects(conn, &secret);

        // Return updated overview
        let servers = db::mcps::list_servers(conn)?;
        let configs = db::mcps::list_configs_display(conn)?;
        let projects = db::projects::list_projects(conn)?;
        let customized_contexts = build_customized_contexts(&configs, &projects);

        Ok(McpOverview { servers, configs, customized_contexts })
    }).await;

    match result {
        Ok(data) => Json(ApiResponse::ok(data)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

// ─── MCP Context Files ──────────────────────────────────────────────────────

/// GET /api/mcps/context/:project_id — list MCP context files for a project
pub async fn list_contexts(
    State(state): State<AppState>,
    Path(project_id): Path<String>,
) -> Json<ApiResponse<Vec<McpContextEntry>>> {
    let result = state.db.with_conn(move |conn| {
        let project = db::projects::get_project(conn, &project_id)?
            .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
        let files = mcp_scanner::list_mcp_context_files(&project.path);
        let entries: Vec<McpContextEntry> = files.into_iter().map(|(slug, label)| {
            let content = mcp_scanner::read_mcp_context(&project.path, &slug)
                .unwrap_or_default();
            McpContextEntry { slug, label, content }
        }).collect();
        Ok(entries)
    }).await;

    match result {
        Ok(entries) => Json(ApiResponse::ok(entries)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// GET /api/mcps/context/:project_id/:slug — read a single MCP context file
pub async fn get_context(
    State(state): State<AppState>,
    Path((project_id, slug)): Path<(String, String)>,
) -> Json<ApiResponse<McpContextEntry>> {
    let result = state.db.with_conn(move |conn| {
        let project = db::projects::get_project(conn, &project_id)?
            .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
        let content = mcp_scanner::read_mcp_context(&project.path, &slug)
            .ok_or_else(|| anyhow::anyhow!("Context file not found"))?;
        Ok(McpContextEntry {
            slug: slug.clone(),
            label: slug.replace('-', " "),
            content,
        })
    }).await;

    match result {
        Ok(entry) => Json(ApiResponse::ok(entry)),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// PUT /api/mcps/context/:project_id/:slug — update a MCP context file
pub async fn update_context(
    State(state): State<AppState>,
    Path((project_id, slug)): Path<(String, String)>,
    Json(req): Json<UpdateMcpContextRequest>,
) -> Json<ApiResponse<()>> {
    let result = state.db.with_conn(move |conn| {
        let project = db::projects::get_project(conn, &project_id)?
            .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
        mcp_scanner::write_mcp_context(&project.path, &slug, &req.content)
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(())
    }).await;

    match result {
        Ok(()) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// Build the `customized_contexts` list: "slug:projectId" pairs where the context
/// file has been customized (not the default template).
fn build_customized_contexts(
    configs: &[McpConfigDisplay],
    projects: &[crate::models::Project],
) -> Vec<String> {
    let mut result = Vec::new();
    for cfg in configs {
        let slug = mcp_scanner::slugify_label(&cfg.label);
        let project_ids: Vec<&str> = if cfg.is_global {
            projects.iter().map(|p| p.id.as_str()).collect()
        } else {
            cfg.project_ids.iter().map(|s| s.as_str()).collect()
        };
        for pid in project_ids {
            if let Some(project) = projects.iter().find(|p| p.id == pid) {
                if let Some(content) = mcp_scanner::read_mcp_context(&project.path, &slug) {
                    if !mcp_scanner::is_default_mcp_context(&content) {
                        result.push(format!("{}:{}", slug, pid));
                    }
                }
            }
        }
    }
    result
}

/// Merge duplicate configs (same config_hash) — keeps the first, merges project linkages, deletes the rest.
fn dedup_configs(conn: &Connection) -> anyhow::Result<()> {
    let configs = db::mcps::list_configs(conn)?;
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new(); // hash → keeper_id
    let mut to_delete: Vec<(String, String)> = vec![]; // (dup_id, keeper_id)

    for config in &configs {
        if let Some(keeper_id) = seen.get(&config.config_hash) {
            to_delete.push((config.id.clone(), keeper_id.clone()));
        } else {
            seen.insert(config.config_hash.clone(), config.id.clone());
        }
    }

    for (dup_id, keeper_id) in &to_delete {
        // Merge project linkages from duplicate into keeper
        let dup = configs.iter().find(|c| c.id == *dup_id).unwrap();
        for pid in &dup.project_ids {
            db::mcps::link_config_project(conn, keeper_id, pid)?;
        }
        // Preserve is_global flag
        if dup.is_global {
            conn.execute(
                "UPDATE mcp_configs SET is_global = 1 WHERE id = ?1",
                params![keeper_id],
            )?;
        }
        // Delete duplicate
        db::mcps::delete_config(conn, dup_id)?;
        tracing::info!("Deduped MCP config {} (merged into {})", dup_id, keeper_id);
    }

    Ok(())
}

/// Recalculate config hashes using current server transports.
/// This prevents hash drift when registry transports are updated.
fn rehash_configs(conn: &Connection, secret: &str) -> anyhow::Result<()> {
    let servers = db::mcps::list_servers(conn)?;
    let configs = db::mcps::list_configs(conn)?;

    let server_map: std::collections::HashMap<String, &McpServer> = servers.iter()
        .map(|s| (s.id.clone(), s))
        .collect();

    for config in &configs {
        let server = match server_map.get(&config.server_id) {
            Some(s) => s,
            None => continue,
        };

        let env = db::mcps::decrypt_env(&config.env_encrypted, secret).unwrap_or_default();
        let new_hash = db::mcps::compute_config_hash(server, &env, config.args_override.as_ref());

        if new_hash != config.config_hash {
            conn.execute(
                "UPDATE mcp_configs SET config_hash = ?1 WHERE id = ?2",
                params![new_hash, config.id],
            )?;
        }
    }

    Ok(())
}

/// Match a detected .mcp.json entry against the built-in registry.
/// Compares command + package name (first non-flag arg) to find the registry entry.
fn match_registry_entry<'a>(
    entry: &mcp_scanner::McpServerEntry,
    reg: &'a [McpDefinition],
) -> Option<&'a McpDefinition> {
    let cmd = entry.command.as_deref()?;
    let args = entry.args.as_deref().unwrap_or(&[]);
    // First non-flag arg is typically the package name
    let pkg = args.iter().find(|a| !a.starts_with('-'))?.as_str();

    reg.iter().find(|def| {
        if let McpTransport::Stdio { command: ref reg_cmd, args: ref reg_args } = def.transport {
            if reg_cmd != cmd {
                return false;
            }
            // Match if the registry package appears in the detected args
            let reg_pkg = reg_args.iter()
                .find(|a| !a.starts_with('-'))
                .map(|s| s.as_str())
                .unwrap_or("");
            // Exact match or detected pkg starts with registry pkg (handles @latest suffix)
            !reg_pkg.is_empty() && (
                pkg == reg_pkg
                || pkg.starts_with(&format!("{}@", reg_pkg))
                || reg_pkg.starts_with(&format!("{}@", pkg))
                // Also match base package name without version
                || strip_version(pkg) == strip_version(reg_pkg)
            )
        } else if let McpTransport::Sse { url: ref reg_url } = def.transport {
            entry.url.as_deref() == Some(reg_url.as_str())
        } else {
            false
        }
    })
}

/// Strip @version suffix from a package name for comparison
fn strip_version(pkg: &str) -> &str {
    // Handle scoped packages like @upstash/context7-mcp@latest
    if let Some(at_pos) = pkg.rfind('@') {
        // Don't strip the scope @ (e.g. @upstash/...)
        if at_pos > 0 {
            return &pkg[..at_pos];
        }
    }
    pkg
}

/// Migrate old `detected:*` servers to registry IDs.
/// Re-points mcp_configs.server_id from the old ID to the registry ID.
fn migrate_detected_to_registry(conn: &Connection, reg: &[McpDefinition]) -> anyhow::Result<()> {
    let servers = db::mcps::list_servers(conn)?;

    for server in &servers {
        if !server.id.starts_with("detected:") {
            continue;
        }

        // Try to match this server's transport against registry
        let matched = reg.iter().find(|def| {
            match (&server.transport, &def.transport) {
                (
                    McpTransport::Stdio { command: ref sc, args: ref sa },
                    McpTransport::Stdio { command: ref rc, args: ref ra },
                ) => {
                    if sc != rc { return false; }
                    let s_pkg = sa.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
                    let r_pkg = ra.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
                    !r_pkg.is_empty() && (
                        s_pkg == r_pkg
                        || strip_version(s_pkg) == strip_version(r_pkg)
                    )
                }
                (
                    McpTransport::Sse { url: ref su },
                    McpTransport::Sse { url: ref ru },
                ) => su == ru,
                _ => false,
            }
        });

        if let Some(def) = matched {
            if def.id == server.id {
                continue; // already correct
            }

            tracing::info!("Migrating MCP server {} -> {}", server.id, def.id);

            // Upsert the registry server
            let new_server = McpServer {
                id: def.id.clone(),
                name: def.name.clone(),
                description: def.description.clone(),
                transport: def.transport.clone(),
                source: McpSource::Registry,
            };
            db::mcps::upsert_server(conn, &new_server)?;

            // Re-point configs from old server_id to new
            conn.execute(
                "UPDATE mcp_configs SET server_id = ?1 WHERE server_id = ?2",
                params![def.id, server.id],
            )?;

            // Delete the old detected server
            conn.execute(
                "DELETE FROM mcp_servers WHERE id = ?1",
                params![server.id],
            )?;
        }
    }

    Ok(())
}
