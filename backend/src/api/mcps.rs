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
    let secret = state.config.read().await.encryption_secret.clone();
    match state.db.with_conn(move |conn| {
        let servers = db::mcps::list_servers(conn)?;
        let configs = db::mcps::list_configs_display(conn, secret.as_deref())?;
        let projects = db::projects::list_projects(conn)?;
        let customized_contexts = build_customized_contexts(&configs, &projects);
        let incompatibilities = mcp_scanner::get_incompatibilities(&servers);

        Ok(McpOverview { servers, configs, customized_contexts, incompatibilities })
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
            let configs = db::mcps::list_configs_display(conn, None)?;
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
            include_general: true,
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

        let configs = db::mcps::list_configs_display(conn, None)?;
        let display = configs.into_iter().find(|c| c.id == config.id)
            .ok_or_else(|| anyhow::anyhow!("Config disappeared after insert"))?;
        Ok(display)
    }).await;

    match result {
        Ok(display) => {
            // Trigger drift detection for affected audited projects
            let pids = if display.is_global {
                projects_for_global(&state).await
            } else {
                display.project_ids.clone()
            };
            trigger_mcp_drift(&state, pids);
            Json(ApiResponse::ok(display))
        }
        Err(e) => Json(ApiResponse::err(format!("{}", e))),
    }
}

/// Helper: get all project IDs (for global MCP changes)
async fn projects_for_global(state: &AppState) -> Vec<String> {
    state.db.with_conn(|conn| {
        Ok(crate::db::projects::list_projects(conn)?
            .into_iter().map(|p| p.id).collect::<Vec<_>>())
    }).await.unwrap_or_default()
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
            req.include_general,
        )?;

        // Sync .mcp.json to disk — always sync all when secrets change
        let secrets_changed = req.env.is_some();
        let global_changed = req.is_global.map(|g| g != old_config.is_global).unwrap_or(false);
        let new_global = req.is_global.unwrap_or(old_config.is_global);
        if secrets_changed || global_changed || new_global {
            // Secrets changed, global flag changed, or is global → sync all projects
            mcp_scanner::sync_all_projects(conn, &secret);
        } else {
            mcp_scanner::sync_affected_projects(conn, &old_config.project_ids, &secret);
        }

        let configs = db::mcps::list_configs_display(conn, None)?;
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

    // Get affected project IDs before deleting
    let affected_pids = state.db.with_conn({
        let cid = config_id.clone();
        move |conn| {
            Ok(db::mcps::get_config(conn, &cid)?
                .map(|c| if c.is_global {
                    crate::db::projects::list_projects(conn).ok()
                        .map(|ps| ps.into_iter().map(|p| p.id).collect::<Vec<_>>())
                        .unwrap_or_default()
                } else {
                    c.project_ids
                })
                .unwrap_or_default())
        }
    }).await.unwrap_or_default();

    match state.db.with_conn(move |conn| {
        let config = db::mcps::get_config(conn, &config_id)?;
        let result = db::mcps::delete_config(conn, &config_id)?;

        if let Some(cfg) = config {
            if cfg.is_global {
                mcp_scanner::sync_all_projects(conn, &secret);
            } else {
                mcp_scanner::sync_affected_projects(conn, &cfg.project_ids, &secret);
            }
        }

        Ok(result)
    }).await {
        Ok(true) => {
            trigger_mcp_drift(&state, affected_pids);
            Json(ApiResponse::ok(()))
        }
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
        let old_config = db::mcps::get_config(conn, &config_id)?;
        let old_pids = old_config.map(|c| c.project_ids).unwrap_or_default();

        db::mcps::set_config_projects(conn, &config_id, &req.project_ids)?;

        let mut all_pids: Vec<String> = old_pids;
        for pid in &req.project_ids {
            if !all_pids.contains(pid) {
                all_pids.push(pid.clone());
            }
        }
        mcp_scanner::sync_affected_projects(conn, &all_pids, &secret);

        Ok(all_pids)
    }).await {
        Ok(all_pids) => {
            trigger_mcp_drift(&state, all_pids);
            Json(ApiResponse::ok(()))
        }
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
                    // SECURITY: a `.mcp.json` from an imported repo can declare ANY
                    // command (e.g. `bash -c '…'`). Kronn syncs this verbatim into
                    // every agent's MCP config and the agent will execute it. We
                    // can't safely block here without breaking custom in-house
                    // MCP servers, but we MUST surface untrusted commands so the
                    // user notices supply-chain risk in their logs.
                    if !is_well_known_mcp_command(cmd) {
                        tracing::warn!(
                            "MCP '{}' in project '{}' uses non-standard command '{}' — \
                             ensure this binary is trusted; .mcp.json from imported repos \
                             can introduce arbitrary code execution.",
                            name, project.name, cmd
                        );
                    }
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
                        include_general: true,
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
        let configs = db::mcps::list_configs_display(conn, None)?;
        let projects = db::projects::list_projects(conn)?;
        let customized_contexts = build_customized_contexts(&configs, &projects);
        let incompatibilities = mcp_scanner::get_incompatibilities(&servers);

        Ok(McpOverview { servers, configs, customized_contexts, incompatibilities })
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

/// Merge duplicate configs — deduplicates by config_hash AND by label+server_id
/// (catches detected:X vs mcp-X pointing to the same MCP).
/// Keeps the first (or the registry-backed one), merges project linkages, deletes the rest.
fn dedup_configs(conn: &Connection) -> anyhow::Result<()> {
    let configs = db::mcps::list_configs(conn)?;
    let mut to_delete: Vec<(String, String)> = vec![]; // (dup_id, keeper_id)

    // Pass 1: same config_hash → exact duplicates
    {
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for config in &configs {
            if let Some(keeper_id) = seen.get(&config.config_hash) {
                to_delete.push((config.id.clone(), keeper_id.clone()));
            } else {
                seen.insert(config.config_hash.clone(), config.id.clone());
            }
        }
    }

    // Pass 2: same label (case-insensitive) + same server_id → duplicates.
    // Covers: detected:X vs mcp-X (prefer registry), both detected (keep first),
    // and both registry with same server_id (keep first — happens after migration).
    {
        // Key: (lowercase_label, server_id) → keeper config_id
        let mut seen: std::collections::HashMap<(String, String), String> = std::collections::HashMap::new();
        let already_deleted: std::collections::HashSet<String> = to_delete.iter().map(|(d, _)| d.clone()).collect();

        for config in &configs {
            if already_deleted.contains(&config.id) { continue; }
            let key = (config.label.to_lowercase(), config.server_id.clone());
            if let Some(keeper_id) = seen.get(&key) {
                to_delete.push((config.id.clone(), keeper_id.clone()));
            } else {
                seen.insert(key, config.id.clone());
            }
        }

        // Also merge detected:X into registry when label matches
        let already_deleted: std::collections::HashSet<String> = to_delete.iter().map(|(d, _)| d.clone()).collect();
        let mut label_to_registry: std::collections::HashMap<String, String> = std::collections::HashMap::new();

        for config in &configs {
            if already_deleted.contains(&config.id) { continue; }
            if !config.server_id.starts_with("detected:") {
                label_to_registry.entry(config.label.to_lowercase())
                    .or_insert_with(|| config.id.clone());
            }
        }
        for config in &configs {
            if already_deleted.contains(&config.id) { continue; }
            if config.server_id.starts_with("detected:") {
                if let Some(keeper_id) = label_to_registry.get(&config.label.to_lowercase()) {
                    to_delete.push((config.id.clone(), keeper_id.clone()));
                }
            }
        }
    }

    for (dup_id, keeper_id) in &to_delete {
        let dup = match configs.iter().find(|c| c.id == *dup_id) {
            Some(d) => d,
            None => continue, // already processed
        };
        for pid in &dup.project_ids {
            db::mcps::link_config_project(conn, keeper_id, pid)?;
        }
        if dup.is_global {
            conn.execute(
                "UPDATE mcp_configs SET is_global = 1 WHERE id = ?1",
                params![keeper_id],
            )?;
        }
        if dup.include_general {
            conn.execute(
                "UPDATE mcp_configs SET include_general = 1 WHERE id = ?1",
                params![keeper_id],
            )?;
        }
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
/// Also checks `alt_packages` so that entries using a different runtime
/// (e.g. npm `fastly-mcp-server` vs Go binary `fastly-mcp`) still match.
fn match_registry_entry<'a>(
    entry: &mcp_scanner::McpServerEntry,
    reg: &'a [McpDefinition],
) -> Option<&'a McpDefinition> {
    let cmd = entry.command.as_deref()?;
    let args = entry.args.as_deref().unwrap_or(&[]);
    // First non-flag arg is typically the package name
    let pkg = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str());

    reg.iter().find(|def| {
        // 1. Check alt_packages: if the detected package matches any alt name,
        //    this is the same MCP regardless of runtime (npx vs binary vs uvx).
        if let Some(detected_pkg) = pkg {
            let stripped = strip_version(detected_pkg);
            if def.alt_packages.iter().any(|alt| {
                stripped == alt.as_str() || strip_version(alt) == stripped
            }) {
                return true;
            }
        }

        // 2. Standard match: same command + matching package name
        if let McpTransport::Stdio { command: ref reg_cmd, args: ref reg_args } = def.transport {
            if reg_cmd != cmd {
                return false;
            }
            let detected_pkg = match pkg {
                Some(p) => p,
                None => return reg_args.is_empty(), // both have no args
            };
            let reg_pkg = reg_args.iter()
                .find(|a| !a.starts_with('-'))
                .map(|s| s.as_str())
                .unwrap_or("");
            !reg_pkg.is_empty() && (
                detected_pkg == reg_pkg
                || detected_pkg.starts_with(&format!("{}@", reg_pkg))
                || reg_pkg.starts_with(&format!("{}@", detected_pkg))
                || strip_version(detected_pkg) == strip_version(reg_pkg)
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

/// Returns true if `command` is a well-known MCP launcher (npx, uvx, python, …).
/// Anything else is allowed but logged as a supply-chain warning so the user
/// notices when an imported `.mcp.json` declares an unusual binary.
///
/// Strips a directory path so absolute commands like `/usr/bin/python3` still
/// match `python3`. Handles both Unix (`/`) and Windows (`\`) separators
/// because the .mcp.json may have been authored on the other platform.
fn is_well_known_mcp_command(cmd: &str) -> bool {
    // Manually find the last path separator — `Path::file_name` only knows
    // about the host OS separator, so on Linux it can't extract the basename
    // from "C:\\Program Files\\nodejs\\node.exe".
    let basename_start = cmd
        .rfind(['/', '\\'])
        .map(|i| i + 1)
        .unwrap_or(0);
    let basename = &cmd[basename_start..];
    let basename = basename
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .trim_end_matches(".ps1");

    matches!(
        basename,
        "npx"
            | "node"
            | "uvx"
            | "uv"
            | "python"
            | "python3"
            | "python3.10"
            | "python3.11"
            | "python3.12"
            | "python3.13"
            | "pipx"
            | "deno"
            | "bun"
            | "bunx"
            | "docker"
            | "podman"
    )
}

#[cfg(test)]
mod command_safety_tests {
    use super::is_well_known_mcp_command;

    #[test]
    fn well_known_launchers_are_accepted() {
        for cmd in ["npx", "uvx", "python3", "node", "deno", "bun"] {
            assert!(is_well_known_mcp_command(cmd), "{} should be well-known", cmd);
        }
    }

    #[test]
    fn absolute_paths_match_basename() {
        assert!(is_well_known_mcp_command("/usr/local/bin/uvx"));
        assert!(is_well_known_mcp_command("/opt/homebrew/bin/python3"));
    }

    #[test]
    fn windows_extensions_are_stripped() {
        assert!(is_well_known_mcp_command("npx.cmd"));
        assert!(is_well_known_mcp_command("C:\\Program Files\\nodejs\\node.exe"));
    }

    #[test]
    fn arbitrary_commands_are_flagged() {
        assert!(!is_well_known_mcp_command("bash"));
        assert!(!is_well_known_mcp_command("sh"));
        assert!(!is_well_known_mcp_command("curl"));
        assert!(!is_well_known_mcp_command("/tmp/evil-binary"));
    }
}

/// Migrate old `detected:*` servers to registry IDs.
/// Re-points mcp_configs.server_id from the old ID to the registry ID.
fn migrate_detected_to_registry(conn: &Connection, reg: &[McpDefinition]) -> anyhow::Result<()> {
    let servers = db::mcps::list_servers(conn)?;

    for server in &servers {
        if !server.id.starts_with("detected:") {
            continue;
        }

        // Try to match this server's transport against registry.
        // Also checks alt_packages for cross-runtime matches (npx pkg vs Go binary).
        let matched = reg.iter().find(|def| {
            // First check alt_packages (handles cross-runtime: npx vs binary)
            if let McpTransport::Stdio { args: ref sa, .. } = server.transport {
                let s_pkg = sa.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("");
                if !s_pkg.is_empty() {
                    let stripped = strip_version(s_pkg);
                    if def.alt_packages.iter().any(|alt| stripped == alt.as_str() || strip_version(alt) == stripped) {
                        return true;
                    }
                }
            }
            // Standard transport match
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

// ── MCP change → auto-reaudit ──────────────────────────────────────────────

/// When an MCP config changes on an audited project, invalidate the .mcp.json
/// checksum so drift detection triggers a step 8 reaudit.
/// Fire-and-forget: spawns a background task, does not block the caller.
fn trigger_mcp_drift(state: &AppState, project_ids: Vec<String>) {
    if project_ids.is_empty() {
        return;
    }
    let db = state.db.clone();
    tokio::spawn(async move {
        let audited = match db.with_conn({
            let pids = project_ids;
            move |conn| {
                let mut result = Vec::new();
                for pid in &pids {
                    if let Ok(Some(p)) = crate::db::projects::get_project(conn, pid) {
                        if p.audit_status == crate::models::AiAuditStatus::Audited || p.audit_status == crate::models::AiAuditStatus::Validated {
                            result.push(p);
                        }
                    }
                }
                Ok(result)
            }
        }).await {
            Ok(ps) => ps,
            Err(e) => { tracing::warn!("MCP drift: failed to query projects: {}", e); return; }
        };

        for project in audited {
            let project_path = crate::core::scanner::resolve_host_path(&project.path);
            let checksums_path = project_path.join("ai/checksums.json");
            if !checksums_path.exists() {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&checksums_path) {
                if let Ok(mut checksums) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(obj) = checksums.as_object_mut() {
                        obj.insert(
                            ".mcp.json".to_string(),
                            serde_json::Value::String("invalidated-by-mcp-change".to_string()),
                        );
                        if let Ok(updated) = serde_json::to_string_pretty(&checksums) {
                            let _ = std::fs::write(&checksums_path, updated);
                            tracing::info!(
                                "MCP change → drift flagged for '{}' (step 8 will re-run on next check)",
                                project.name
                            );
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp_scanner::McpServerEntry;

    fn make_entry(cmd: &str, args: &[&str]) -> McpServerEntry {
        McpServerEntry {
            command: Some(cmd.into()),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            url: None,
            env: Default::default(),
        }
    }

    fn make_def(id: &str, cmd: &str, args: &[&str], alt: &[&str]) -> McpDefinition {
        McpDefinition {
            id: id.into(),
            name: id.into(),
            description: String::new(),
            transport: McpTransport::Stdio {
                command: cmd.into(),
                args: args.iter().map(|s| s.to_string()).collect(),
            },
            env_keys: vec![],
            tags: vec![],
            token_url: None,
            token_help: None,
            publisher: String::new(),
            official: false,
            alt_packages: alt.iter().map(|s| s.to_string()).collect(),
            default_context: None,
        }
    }

    #[test]
    fn match_registry_exact_command_and_package() {
        let reg = vec![make_def("mcp-github", "npx", &["-y", "@modelcontextprotocol/server-github"], &[])];
        let entry = make_entry("npx", &["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(match_registry_entry(&entry, &reg).unwrap().id, "mcp-github");
    }

    #[test]
    fn match_registry_versioned_package() {
        let reg = vec![make_def("mcp-github", "npx", &["-y", "@modelcontextprotocol/server-github"], &[])];
        let entry = make_entry("npx", &["-y", "@modelcontextprotocol/server-github@latest"]);
        assert_eq!(match_registry_entry(&entry, &reg).unwrap().id, "mcp-github");
    }

    #[test]
    fn match_registry_alt_package_cross_runtime() {
        // Registry uses Go binary, .mcp.json uses npm package
        let reg = vec![make_def("mcp-fastly", "fastly-mcp", &[], &["fastly-mcp-server"])];
        let entry = make_entry("npx", &["-y", "fastly-mcp-server@1.0.4"]);
        assert_eq!(match_registry_entry(&entry, &reg).unwrap().id, "mcp-fastly");
    }

    #[test]
    fn match_registry_alt_package_gitlab() {
        // Registry uses glab CLI, .mcp.json uses npm package
        let reg = vec![make_def("mcp-gitlab", "glab", &["mcp", "serve"], &["@modelcontextprotocol/server-gitlab"])];
        let entry = make_entry("npx", &["-y", "@modelcontextprotocol/server-gitlab"]);
        assert_eq!(match_registry_entry(&entry, &reg).unwrap().id, "mcp-gitlab");
    }

    #[test]
    fn match_registry_no_match_different_package() {
        let reg = vec![make_def("mcp-github", "npx", &["-y", "@modelcontextprotocol/server-github"], &[])];
        let entry = make_entry("npx", &["-y", "some-other-server"]);
        assert!(match_registry_entry(&entry, &reg).is_none());
    }

    #[test]
    fn match_registry_uvx_exact() {
        let reg = vec![make_def("mcp-docker", "uvx", &["mcp-server-docker"], &[])];
        let entry = make_entry("uvx", &["mcp-server-docker"]);
        assert_eq!(match_registry_entry(&entry, &reg).unwrap().id, "mcp-docker");
    }

    #[test]
    fn strip_version_scoped_package() {
        assert_eq!(strip_version("@upstash/context7-mcp@latest"), "@upstash/context7-mcp");
        assert_eq!(strip_version("fastly-mcp-server@1.0.4"), "fastly-mcp-server");
        assert_eq!(strip_version("@modelcontextprotocol/server-gitlab"), "@modelcontextprotocol/server-gitlab");
    }
}
