//! Versioned plugin selection export/import.
//!
//! Configuration-only bundles are clear JSON and never contain environment
//! values. Opting into values requires a typed confirmation and passphrase;
//! the full payload is then encrypted under a random AES-256-GCM key, itself
//! wrapped with Kronn's Argon2id recovery framing.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    core::{crypto, recovery, registry},
    db,
    models::{
        ApiAuthKind, ApiErrorCode, ApiResponse, HostSyncMode, McpConfig, McpServer, McpSource,
        McpTransport, PluginInterface,
    },
    AppState,
};

const PLUGIN_BUNDLE_KIND: &str = "kronn.plugins";
const PLUGIN_BUNDLE_VERSION: u32 = 1;
const SECRET_CONFIRMATION: &str = "EXPORTER LES SECRETS";
const MIN_PASSPHRASE_LEN: usize = 12;
const MAX_PLUGIN_SELECTION: usize = 100;

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundleSelectionRequest {
    pub config_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundleValueDescriptor {
    pub key: String,
    pub sensitive: bool,
    pub exportable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundlePreviewItem {
    pub config_id: String,
    pub server_id: String,
    pub label: String,
    pub server_name: String,
    pub cli_credential: bool,
    pub values: Vec<PluginBundleValueDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundlePreview {
    pub plugins: Vec<PluginBundlePreviewItem>,
    pub value_count: u32,
    pub sensitive_value_count: u32,
    pub confirmation_phrase: String,
    pub minimum_passphrase_length: u32,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ExportPluginBundleRequest {
    pub config_ids: Vec<String>,
    #[serde(default)]
    pub include_values: bool,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub confirmation: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ImportPluginBundleRequest {
    pub content: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Source config ids whose bundled custom arguments the importer accepted.
    /// They replace the plugin's whole command line, so every other plugin's
    /// arguments are dropped.
    #[serde(default)]
    pub accept_args_for: Vec<String>,
}

/// One plugin as the import would treat it, before anything is written.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ImportPreviewPlugin {
    pub source_config_id: String,
    pub label: String,
    pub server_name: String,
    /// The command line the plugin runs today (catalogue), for stdio plugins.
    pub usual_args: Option<Vec<String>>,
    /// The bundle's proposed command line, secret-looking parts masked.
    pub proposed_args: Option<Vec<String>>,
    pub args_differ: bool,
    pub importable: bool,
    pub issue: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ImportBundlePreview {
    pub bundle_id: String,
    pub already_imported: bool,
    pub includes_values: bool,
    /// The file was a single-plugin JSON from the old per-plugin export.
    pub legacy: bool,
    pub plugins: Vec<ImportPreviewPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PortablePluginConfig {
    pub source_config_id: String,
    pub server: McpServer,
    pub label: String,
    pub env_keys: Vec<String>,
    #[ts(type = "Record<string, string> | null")]
    pub values: Option<BTreeMap<String, String>>,
    pub args_override: Option<Vec<String>>,
    pub was_global: bool,
    pub include_general: bool,
    pub preferred_interface: PluginInterface,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundlePayload {
    pub plugins: Vec<PortablePluginConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PluginBundleEnvelope {
    pub kind: String,
    pub version: u32,
    pub bundle_id: String,
    pub exported_at: DateTime<Utc>,
    pub includes_values: bool,
    pub encrypted: bool,
    pub plugin_labels: Vec<String>,
    pub value_manifest: Vec<PluginBundlePreviewItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<PluginBundlePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportedPluginConfig {
    pub config_id: String,
    pub server_id: String,
    pub label: String,
    pub server_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportPluginBundleReport {
    pub bundle_id: String,
    pub already_imported: bool,
    pub imported_config_ids: Vec<String>,
    /// Human-readable rows for the post-import scope assignment UI. Kept in
    /// addition to `imported_config_ids` for backwards API compatibility.
    #[serde(default)]
    pub imported_configs: Vec<ImportedPluginConfig>,
    pub skipped_plugins: u32,
    pub includes_values: bool,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
}

fn unique_selection(config_ids: &[String]) -> anyhow::Result<Vec<String>> {
    if config_ids.is_empty() {
        anyhow::bail!("Select at least one plugin");
    }
    if config_ids.len() > MAX_PLUGIN_SELECTION {
        anyhow::bail!("A bundle can contain at most {MAX_PLUGIN_SELECTION} plugins");
    }
    let mut seen = HashSet::new();
    let ids = config_ids
        .iter()
        .filter_map(|id| {
            let id = id.trim();
            (!id.is_empty() && seen.insert(id.to_string())).then(|| id.to_string())
        })
        .collect::<Vec<_>>();
    if ids.is_empty() {
        anyhow::bail!("Select at least one plugin");
    }
    Ok(ids)
}

fn auth_secret_keys(auth: &ApiAuthKind) -> HashSet<String> {
    match auth {
        ApiAuthKind::ApiKeyQuery { env_key, .. }
        | ApiAuthKind::ApiKeyHeader { env_key, .. }
        | ApiAuthKind::Bearer { env_key }
        | ApiAuthKind::BasicApiKey { env_key } => HashSet::from([env_key.clone()]),
        ApiAuthKind::Basic {
            user_env,
            password_env,
        } => HashSet::from([user_env.clone(), password_env.clone()]),
        ApiAuthKind::OAuth2ClientCredentials {
            client_id_env,
            client_secret_env,
            ..
        } => HashSet::from([client_id_env.clone(), client_secret_env.clone()]),
        ApiAuthKind::TokenExchange { creds_env_keys, .. } => {
            creds_env_keys.iter().cloned().collect()
        }
        ApiAuthKind::CliToken { .. } | ApiAuthKind::None => HashSet::new(),
    }
}

fn plugin_value_descriptors(
    server: &McpServer,
    config: &McpConfig,
) -> Vec<PluginBundleValueDescriptor> {
    let cli_credential = server
        .api_spec
        .as_ref()
        .is_some_and(|spec| matches!(spec.auth, ApiAuthKind::CliToken { .. }));
    let config_keys: HashSet<String> = server
        .api_spec
        .as_ref()
        .map(|spec| {
            spec.config_keys
                .iter()
                .map(|key| key.env_key.clone())
                .collect()
        })
        .unwrap_or_default();
    let auth_keys = server
        .api_spec
        .as_ref()
        .map(|spec| auth_secret_keys(&spec.auth))
        .unwrap_or_default();

    let mut values = config
        .env_keys
        .iter()
        .map(|key| {
            let sensitive = auth_keys.contains(key) || !config_keys.contains(key);
            PluginBundleValueDescriptor {
                key: key.clone(),
                sensitive,
                // A CLI credential is resolved live and must never be copied.
                // Non-secret instance parameters remain portable.
                exportable: !cli_credential || !sensitive,
            }
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.key.cmp(&right.key));
    values
}

fn load_selection(
    conn: &rusqlite::Connection,
    config_ids: &[String],
) -> anyhow::Result<Vec<(McpConfig, McpServer, PluginInterface)>> {
    let ids = unique_selection(config_ids)?;
    let configs = db::mcps::list_configs(conn)?;
    let servers = db::mcps::list_servers(conn)?;
    let preferences = db::mcps::list_config_preferences(conn)?;
    let config_map: HashMap<_, _> = configs
        .into_iter()
        .map(|config| (config.id.clone(), config))
        .collect();
    let server_map: HashMap<_, _> = servers
        .into_iter()
        .map(|server| (server.id.clone(), server))
        .collect();

    ids.into_iter()
        .map(|id| {
            let config = config_map
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Plugin config `{id}` was not found"))?;
            let server = server_map
                .get(&config.server_id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Server `{}` was not found", config.server_id))?;
            let preference = preferences.get(&id).copied().unwrap_or_default();
            Ok((config, server, preference))
        })
        .collect()
}

fn build_preview(selection: &[(McpConfig, McpServer, PluginInterface)]) -> PluginBundlePreview {
    let plugins = selection
        .iter()
        .map(|(config, server, _)| {
            let cli_credential = server
                .api_spec
                .as_ref()
                .is_some_and(|spec| matches!(spec.auth, ApiAuthKind::CliToken { .. }));
            PluginBundlePreviewItem {
                config_id: config.id.clone(),
                server_id: server.id.clone(),
                label: config.label.clone(),
                server_name: server.name.clone(),
                cli_credential,
                values: plugin_value_descriptors(server, config),
            }
        })
        .collect::<Vec<_>>();
    let value_count = plugins
        .iter()
        .flat_map(|plugin| &plugin.values)
        .filter(|value| value.exportable)
        .count() as u32;
    let sensitive_value_count = plugins
        .iter()
        .flat_map(|plugin| &plugin.values)
        .filter(|value| value.exportable && value.sensitive)
        .count() as u32;
    PluginBundlePreview {
        plugins,
        value_count,
        sensitive_value_count,
        confirmation_phrase: SECRET_CONFIRMATION.into(),
        minimum_passphrase_length: MIN_PASSPHRASE_LEN as u32,
    }
}

fn plugin_bundle_filename(labels: &[String]) -> String {
    let stem = if labels.len() == 1 {
        labels[0].clone()
    } else {
        format!("{}-plugins", labels.len())
    };
    let safe = stem
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let safe = safe.trim_matches('-');
    format!(
        "{}.kronn-plugins.json",
        if safe.is_empty() { "plugins" } else { safe }
    )
}

fn record_event(
    conn: &rusqlite::Connection,
    action: &str,
    bundle_id: &str,
    config_ids: &[String],
    includes_values: bool,
    success: bool,
    detail: &serde_json::Value,
) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO plugin_bundle_events
         (id, action, bundle_id, config_ids_json, includes_values, success, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            Uuid::new_v4().to_string(),
            action,
            bundle_id,
            serde_json::to_string(config_ids)?,
            includes_values as i32,
            success as i32,
            serde_json::to_string(detail)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

fn build_export(
    conn: &rusqlite::Connection,
    request: ExportPluginBundleRequest,
    instance_secret: &str,
) -> anyhow::Result<(PluginBundleEnvelope, String)> {
    let selection = load_selection(conn, &request.config_ids)?;
    let preview = build_preview(&selection);
    if request.include_values {
        if request.confirmation.as_deref() != Some(SECRET_CONFIRMATION) {
            anyhow::bail!("Type `{SECRET_CONFIRMATION}` to include values");
        }
        let passphrase = request.passphrase.as_deref().unwrap_or_default();
        if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
            anyhow::bail!(
                "The export passphrase must contain at least {MIN_PASSPHRASE_LEN} characters"
            );
        }
    }

    let mut portable = Vec::with_capacity(selection.len());
    for (config, server, preference) in &selection {
        let descriptors = plugin_value_descriptors(server, config);
        let values = if request.include_values {
            let decrypted =
                db::mcps::decrypt_env(&config.env_encrypted, instance_secret).map_err(|error| {
                    anyhow::anyhow!("Cannot decrypt values for `{}`: {error}", config.label)
                })?;
            Some(
                descriptors
                    .iter()
                    .filter(|descriptor| descriptor.exportable)
                    .filter_map(|descriptor| {
                        decrypted
                            .get(&descriptor.key)
                            .map(|value| (descriptor.key.clone(), value.clone()))
                    })
                    .collect(),
            )
        } else {
            None
        };
        portable.push(PortablePluginConfig {
            source_config_id: config.id.clone(),
            server: server.clone(),
            label: config.label.clone(),
            env_keys: config.env_keys.clone(),
            values,
            // Arguments may embed tokens: only the encrypted payload carries them.
            args_override: if request.include_values {
                config.args_override.clone()
            } else {
                None
            },
            was_global: config.is_global,
            include_general: config.include_general,
            preferred_interface: *preference,
        });
    }

    let payload = PluginBundlePayload { plugins: portable };
    let bundle_id = Uuid::new_v4().to_string();
    let labels = preview
        .plugins
        .iter()
        .map(|plugin| plugin.label.clone())
        .collect::<Vec<_>>();
    let (clear_payload, encrypted_payload, wrapped_key) = if request.include_values {
        let passphrase = request.passphrase.as_deref().unwrap_or_default();
        let key_hex = crypto::generate_secret();
        let key = crypto::parse_secret(&key_hex).map_err(anyhow::Error::msg)?;
        let plaintext = serde_json::to_string(&payload)?;
        let ciphertext = crypto::encrypt(&plaintext, &key).map_err(anyhow::Error::msg)?;
        let wrapped = recovery::wrap_key(&key_hex, passphrase).map_err(anyhow::Error::msg)?;
        (None, Some(ciphertext), Some(recovery::to_code(&wrapped)))
    } else {
        (Some(payload), None, None)
    };
    let envelope = PluginBundleEnvelope {
        kind: PLUGIN_BUNDLE_KIND.into(),
        version: PLUGIN_BUNDLE_VERSION,
        bundle_id: bundle_id.clone(),
        exported_at: Utc::now(),
        includes_values: request.include_values,
        encrypted: request.include_values,
        plugin_labels: labels.clone(),
        value_manifest: preview.plugins,
        payload: clear_payload,
        encrypted_payload,
        wrapped_key,
    };
    let config_ids = selection
        .iter()
        .map(|(config, _, _)| config.id.clone())
        .collect::<Vec<_>>();
    record_event(
        conn,
        "export",
        &bundle_id,
        &config_ids,
        request.include_values,
        true,
        &serde_json::json!({
            "plugin_count": config_ids.len(),
            "value_count": preview.value_count,
            "sensitive_value_count": preview.sensitive_value_count,
        }),
    )?;
    Ok((envelope, plugin_bundle_filename(&labels)))
}

fn decode_payload(
    envelope: &PluginBundleEnvelope,
    passphrase: Option<&str>,
) -> anyhow::Result<PluginBundlePayload> {
    if envelope.encrypted || envelope.includes_values {
        let passphrase = passphrase
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("A passphrase is required to import this encrypted bundle")
            })?;
        let wrapped = envelope
            .wrapped_key
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Encrypted bundle is missing its wrapped key"))?;
        let ciphertext = envelope
            .encrypted_payload
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Encrypted bundle is missing its payload"))?;
        if envelope.payload.is_some() {
            anyhow::bail!("Encrypted bundles cannot contain a clear payload");
        }
        let blob = recovery::from_code(wrapped).map_err(anyhow::Error::msg)?;
        let key_hex = recovery::unwrap_key(&blob, passphrase).map_err(anyhow::Error::msg)?;
        let key = crypto::parse_secret(&key_hex).map_err(anyhow::Error::msg)?;
        let plaintext = crypto::decrypt(ciphertext, &key)
            .map_err(|_| anyhow::anyhow!("Wrong passphrase or corrupt encrypted plugin bundle"))?;
        serde_json::from_str(&plaintext).map_err(anyhow::Error::from)
    } else {
        if envelope.encrypted_payload.is_some() || envelope.wrapped_key.is_some() {
            anyhow::bail!("Clear bundle contains unexpected encryption fields");
        }
        envelope
            .payload
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Plugin bundle payload is missing"))
    }
}

fn validate_payload_contract(
    envelope: &PluginBundleEnvelope,
    payload: &PluginBundlePayload,
) -> anyhow::Result<()> {
    if envelope.encrypted != envelope.includes_values {
        anyhow::bail!("Plugin bundle encryption flags are inconsistent");
    }
    if payload.plugins.is_empty() {
        anyhow::bail!("Plugin bundle contains no plugins");
    }
    if payload.plugins.len() > MAX_PLUGIN_SELECTION {
        anyhow::bail!("Plugin bundle contains too many plugins");
    }
    let mut source_ids = HashSet::new();
    for plugin in &payload.plugins {
        if plugin.source_config_id.trim().is_empty()
            || !source_ids.insert(plugin.source_config_id.clone())
        {
            anyhow::bail!("Plugin bundle contains an empty or duplicate source config id");
        }
        if !envelope.includes_values
            && plugin
                .values
                .as_ref()
                .is_some_and(|values| !values.is_empty())
        {
            anyhow::bail!("Clear plugin bundles cannot contain environment values");
        }
    }
    Ok(())
}

fn semantic_fingerprint(envelope: &PluginBundleEnvelope) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(envelope)?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn current_registry_server(server_id: &str) -> Option<McpServer> {
    registry::builtin_registry()
        .into_iter()
        .find(|definition| definition.id == server_id)
        .map(|definition| McpServer {
            id: definition.id,
            name: definition.name,
            description: definition.description,
            transport: definition.transport,
            source: McpSource::Registry,
            api_spec: definition.api_spec,
        })
}

fn is_safe_manual_server(server: &McpServer) -> bool {
    matches!(server.transport, McpTransport::ApiOnly)
        && !server
            .api_spec
            .as_ref()
            .is_some_and(|spec| matches!(spec.auth, ApiAuthKind::CliToken { .. }))
}

fn allowed_import_env_keys(server: &McpServer) -> HashSet<String> {
    if let Some(definition) = registry::builtin_registry()
        .into_iter()
        .find(|definition| definition.id == server.id)
    {
        return definition.env_keys.into_iter().collect();
    }
    server
        .api_spec
        .as_ref()
        .map(|spec| {
            let mut keys = auth_secret_keys(&spec.auth);
            keys.extend(spec.config_keys.iter().map(|key| key.env_key.clone()));
            keys
        })
        .unwrap_or_default()
}

/// Built-in catalogue ids use these prefixes; a manual server must not take
/// one, or a later catalogue refresh would overwrite it.
fn has_reserved_registry_prefix(server_id: &str) -> bool {
    server_id.starts_with("mcp-") || server_id.starts_with("api-")
}

/// The custom arguments an import may apply: none unless the importer gave
/// explicit consent, since they replace the trusted command's arguments.
/// Arguments equal to the server's own are a no-op and are dropped silently.
fn import_args_override(
    server: &McpServer,
    portable: &PortablePluginConfig,
    accept_args_override: bool,
    conflicts: &mut Vec<String>,
) -> Option<Vec<String>> {
    let requested = portable.args_override.as_ref()?;
    let own_args = match &server.transport {
        McpTransport::Stdio { args, .. } => Some(args),
        _ => None,
    };
    if own_args == Some(requested) {
        return None;
    }
    if own_args.is_none() {
        conflicts.push(format!(
            "{}: custom arguments do not apply to this plugin and were discarded",
            portable.label
        ));
        return None;
    }
    if accept_args_override {
        return Some(requested.clone());
    }
    conflicts.push(format!(
        "{}: {} custom argument(s) were not applied because they replace the plugin's command line; review them and set them by hand, or import again with explicit consent",
        portable.label,
        requested.len()
    ));
    None
}

fn resolve_import_server(
    conn: &rusqlite::Connection,
    portable: &PortablePluginConfig,
    warnings: &mut Vec<String>,
    conflicts: &mut Vec<String>,
) -> anyhow::Result<Option<McpServer>> {
    if let Some(registry_server) = current_registry_server(&portable.server.id) {
        db::mcps::upsert_server(conn, &registry_server)?;
        if serde_json::to_value(&registry_server)? != serde_json::to_value(&portable.server)? {
            warnings.push(format!(
                "{}: current trusted registry definition replaced the bundled snapshot",
                portable.label
            ));
        }
        return Ok(Some(registry_server));
    }

    if has_reserved_registry_prefix(&portable.server.id) {
        conflicts.push(format!(
            "{}: server id `{}` is reserved for the built-in catalogue and is not in it; skipped",
            portable.label, portable.server.id
        ));
        return Ok(None);
    }

    if !is_safe_manual_server(&portable.server) {
        conflicts.push(format!(
            "{}: unknown executable/MCP server definitions cannot be imported; install the trusted plugin first",
            portable.label
        ));
        return Ok(None);
    }

    if let Some(existing) = db::mcps::list_servers(conn)?
        .into_iter()
        .find(|server| server.id == portable.server.id)
    {
        if serde_json::to_value(&existing)? != serde_json::to_value(&portable.server)? {
            conflicts.push(format!(
                "{}: server id `{}` already exists with a different definition",
                portable.label, portable.server.id
            ));
            return Ok(None);
        }
        return Ok(Some(existing));
    }

    let mut server = portable.server.clone();
    server.source = McpSource::Manual;
    db::mcps::upsert_server(conn, &server)?;
    Ok(Some(server))
}

/// A re-import of an already imported bundle may add the consent the first
/// import lacked: the accepted plugins' arguments are applied to the configs
/// that import created. Nothing else is touched.
fn apply_late_consent(
    conn: &rusqlite::Connection,
    payload: &PluginBundlePayload,
    consent: &HashSet<String>,
    instance_secret: &str,
    report: &mut ImportPluginBundleReport,
) -> anyhow::Result<()> {
    for portable in &payload.plugins {
        if !consent.contains(&portable.source_config_id) {
            continue;
        }
        let Some(server) = current_registry_server(&portable.server.id).or_else(|| {
            db::mcps::list_servers(conn)
                .ok()?
                .into_iter()
                .find(|server| server.id == portable.server.id)
        }) else {
            continue;
        };
        let mut ignored = Vec::new();
        let Some(args) = import_args_override(&server, portable, true, &mut ignored) else {
            continue;
        };
        let Some(config) = report
            .imported_configs
            .iter()
            .find(|item| item.server_id == server.id && item.label == portable.label)
            .and_then(|item| db::mcps::get_config(conn, &item.config_id).ok().flatten())
        else {
            continue;
        };
        if config.args_override.as_ref() == Some(&args) {
            continue;
        }
        let env = db::mcps::decrypt_env(&config.env_encrypted, instance_secret)
            .map_err(anyhow::Error::msg)?;
        let hash = db::mcps::compute_config_hash(&server, &env, Some(&args));
        db::mcps::update_config(
            conn,
            &config.id,
            None,
            None,
            None,
            Some(&args),
            None,
            Some(&hash),
            None,
            None,
            None,
        )?;
        report.warnings.push(format!(
            "{}: the accepted custom arguments were applied",
            portable.label
        ));
    }
    Ok(())
}

fn import_payload(
    conn: &rusqlite::Connection,
    envelope: &PluginBundleEnvelope,
    payload: PluginBundlePayload,
    instance_secret: &str,
    fingerprint: &str,
    consent: &HashSet<String>,
) -> anyhow::Result<ImportPluginBundleReport> {
    if let Some((existing_hash, report_json)) = conn
        .query_row(
            "SELECT content_sha256, report_json
             FROM plugin_bundle_imports WHERE source_bundle_id = ?1",
            [&envelope.bundle_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if existing_hash != fingerprint {
            anyhow::bail!(
                "IMPORT_CONFLICT: bundle {} was already imported from different content",
                envelope.bundle_id
            );
        }
        let mut report: ImportPluginBundleReport = serde_json::from_str(&report_json)?;
        report.already_imported = true;
        if !consent.is_empty() {
            apply_late_consent(conn, &payload, consent, instance_secret, &mut report)?;
        }
        return Ok(report);
    }

    let transaction = conn.unchecked_transaction()?;
    let mut imported_config_ids = Vec::new();
    let mut imported_configs = Vec::new();
    let mut warnings = Vec::new();
    let mut conflicts = Vec::new();
    let mut skipped_plugins = 0_u32;
    let mut existing_configs = db::mcps::list_configs(&transaction)?;

    for portable in payload.plugins {
        let Some(server) =
            resolve_import_server(&transaction, &portable, &mut warnings, &mut conflicts)?
        else {
            skipped_plugins += 1;
            continue;
        };
        let args_override = import_args_override(
            &server,
            &portable,
            consent.contains(&portable.source_config_id),
            &mut conflicts,
        );
        let allowed_env_keys = allowed_import_env_keys(&server);
        let env_keys = portable
            .env_keys
            .iter()
            .filter(|key| {
                if allowed_env_keys.contains(*key) {
                    true
                } else {
                    conflicts.push(format!(
                        "{}: undeclared environment key `{key}` was discarded",
                        portable.label
                    ));
                    false
                }
            })
            .cloned()
            .collect::<Vec<_>>();
        if existing_configs
            .iter()
            .any(|config| config.server_id == server.id && config.label == portable.label)
        {
            conflicts.push(format!(
                "{}: a configuration with the same plugin and label already exists; skipped",
                portable.label
            ));
            skipped_plugins += 1;
            continue;
        }

        let descriptors = plugin_value_descriptors(
            &server,
            &McpConfig {
                id: portable.source_config_id.clone(),
                server_id: server.id.clone(),
                label: portable.label.clone(),
                env_keys: env_keys.clone(),
                env_encrypted: String::new(),
                args_override: args_override.clone(),
                is_global: false,
                include_general: portable.include_general,
                config_hash: String::new(),
                project_ids: Vec::new(),
                // Throwaway config, only used to compute value descriptors
                // below — never inserted, so `host_sync` here is inert.
                host_sync: HostSyncMode::None,
            },
        );
        let allowed_keys: HashSet<_> = descriptors
            .iter()
            .filter(|descriptor| descriptor.exportable)
            .map(|descriptor| descriptor.key.as_str())
            .collect();
        let supplied = portable.values.unwrap_or_default();
        let mut env = HashMap::new();
        for key in &env_keys {
            if let Some(value) = supplied.get(key) {
                if !allowed_keys.contains(key.as_str()) {
                    conflicts.push(format!(
                        "{}: value `{key}` is CLI-backed/non-exportable and was discarded",
                        portable.label
                    ));
                    env.insert(key.clone(), String::new());
                } else {
                    env.insert(key.clone(), value.clone());
                }
            } else {
                env.insert(key.clone(), String::new());
            }
        }
        for unknown in supplied.keys().filter(|key| !env.contains_key(*key)) {
            conflicts.push(format!(
                "{}: undeclared value `{unknown}` was discarded",
                portable.label
            ));
        }

        let hash = db::mcps::compute_config_hash(&server, &env, args_override.as_ref());
        if existing_configs
            .iter()
            .any(|config| config.config_hash == hash)
        {
            conflicts.push(format!(
                "{}: an equivalent configuration already exists; skipped",
                portable.label
            ));
            skipped_plugins += 1;
            continue;
        }
        let config_id = Uuid::new_v4().to_string();
        let encrypted = db::mcps::encrypt_env(&env, instance_secret).map_err(anyhow::Error::msg)?;
        let imported_server_id = server.id.clone();
        let imported_server_name = server.name.clone();
        let config = McpConfig {
            id: config_id.clone(),
            server_id: imported_server_id.clone(),
            label: portable.label.clone(),
            env_keys,
            env_encrypted: encrypted,
            args_override,
            // Never broaden project/host exposure during import: `is_global`
            // is reset above, and `host_sync` below is the documented
            // default — `PortablePluginConfig` never carries a host_sync
            // value (it's host-machine-specific, not portable across
            // machines), so an import always starts unsynced regardless of
            // what the source config's own host_sync was.
            is_global: false,
            include_general: portable.include_general,
            config_hash: hash,
            project_ids: Vec::new(),
            host_sync: HostSyncMode::None,
        };
        db::mcps::insert_config(&transaction, &config)?;
        db::mcps::update_config(
            &transaction,
            &config_id,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(portable.preferred_interface),
        )?;
        if portable.was_global {
            warnings.push(format!(
                "{}: imported as unscoped instead of global; choose its scope explicitly",
                portable.label
            ));
        }
        existing_configs.push(config);
        imported_config_ids.push(config_id.clone());
        imported_configs.push(ImportedPluginConfig {
            config_id,
            server_id: imported_server_id,
            label: portable.label,
            server_name: imported_server_name,
        });
    }

    let report = ImportPluginBundleReport {
        bundle_id: envelope.bundle_id.clone(),
        already_imported: false,
        skipped_plugins,
        includes_values: envelope.includes_values,
        imported_config_ids: imported_config_ids.clone(),
        imported_configs,
        warnings,
        conflicts,
    };
    transaction.execute(
        "INSERT INTO plugin_bundle_imports
         (source_bundle_id, content_sha256, report_json, imported_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            envelope.bundle_id,
            fingerprint,
            serde_json::to_string(&report)?,
            Utc::now().to_rfc3339(),
        ],
    )?;
    record_event(
        &transaction,
        "import",
        &envelope.bundle_id,
        &imported_config_ids,
        envelope.includes_values,
        true,
        &serde_json::json!({
            "imported": report.imported_config_ids.len(),
            "warnings": report.warnings.len(),
            "conflicts": report.conflicts.len(),
        }),
    )?;
    transaction.commit()?;
    Ok(report)
}

fn api_error(status: StatusCode, code: ApiErrorCode, message: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::err_coded(code, message))).into_response()
}

/// POST /api/mcps/bundles/preview
pub async fn preview_plugin_bundle(
    State(state): State<AppState>,
    Json(request): Json<PluginBundleSelectionRequest>,
) -> Json<ApiResponse<PluginBundlePreview>> {
    match state
        .db
        .with_read_conn(move |conn| {
            let selection = load_selection(conn, &request.config_ids)?;
            Ok(build_preview(&selection))
        })
        .await
    {
        Ok(preview) => Json(ApiResponse::ok(preview)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        )),
    }
}

/// POST /api/mcps/bundles/export
pub async fn export_plugin_bundle(
    State(state): State<AppState>,
    Json(request): Json<ExportPluginBundleRequest>,
) -> Response {
    let instance_secret = match state.config.read().await.encryption_secret.clone() {
        Some(secret) => secret,
        None => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiErrorCode::Internal,
                "No encryption secret configured",
            )
        }
    };
    let result = state
        .db
        .with_conn(move |conn| build_export(conn, request, &instance_secret))
        .await;
    let (envelope, filename) = match result {
        Ok(result) => result,
        Err(error) => {
            return api_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                ApiErrorCode::Validation,
                error.to_string(),
            )
        }
    };
    match serde_json::to_string_pretty(&envelope) {
        Ok(body) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    "application/json; charset=utf-8".to_string(),
                ),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                ),
            ],
            body,
        )
            .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ApiErrorCode::Internal,
            format!("Could not serialize plugin bundle: {error}"),
        ),
    }
}

/// Parse an uploaded file as a plugin bundle. A single-plugin JSON from the old
/// per-plugin export is converted into a one-plugin bundle here, so it goes
/// through exactly the same validation as any bundle.
fn load_bundle(
    content: &str,
    passphrase: Option<&str>,
) -> Result<(PluginBundleEnvelope, PluginBundlePayload, String, bool), (ApiErrorCode, String)> {
    let validation = |message: String| (ApiErrorCode::Validation, message);
    let raw: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| validation(format!("Invalid plugin bundle JSON: {error}")))?;
    let (envelope, legacy_payload) = if raw.get("kind").is_none() && raw.get("name").is_some() {
        let (envelope, payload) = legacy_bundle(raw, content).map_err(validation)?;
        (envelope, Some(payload))
    } else {
        let envelope: PluginBundleEnvelope = serde_json::from_value(raw)
            .map_err(|error| validation(format!("Invalid plugin bundle JSON: {error}")))?;
        (envelope, None)
    };
    let legacy = legacy_payload.is_some();
    if envelope.kind != PLUGIN_BUNDLE_KIND || envelope.version != PLUGIN_BUNDLE_VERSION {
        return Err(validation(format!(
            "Unsupported plugin bundle (kind `{}`, version {})",
            envelope.kind, envelope.version
        )));
    }
    if envelope.bundle_id.trim().is_empty() {
        return Err(validation("Plugin bundle id is required".into()));
    }
    let payload = match legacy_payload {
        Some(payload) => payload,
        None => decode_payload(&envelope, passphrase).map_err(|e| validation(e.to_string()))?,
    };
    validate_payload_contract(&envelope, &payload).map_err(|e| validation(e.to_string()))?;
    let fingerprint =
        semantic_fingerprint(&envelope).map_err(|e| (ApiErrorCode::Internal, e.to_string()))?;
    Ok((envelope, payload, fingerprint, legacy))
}

/// The old per-plugin JSON (`name`, `base_url`, `fields`, `endpoints`, `auth`)
/// as a one-plugin bundle. Values are never carried, as before.
fn legacy_bundle(
    raw: serde_json::Value,
    content: &str,
) -> Result<(PluginBundleEnvelope, PluginBundlePayload), String> {
    let spec: crate::models::CustomApiPayload = serde_json::from_value(raw)
        .map_err(|error| format!("Not a plugin bundle or a plugin export: {error}"))?;
    let spec = crate::api::mcps::sanitize_imported_payload(spec)?;
    let server = crate::api::mcps::materialize_custom_server(&spec);
    let env_keys = spec
        .fields
        .iter()
        .filter(|field| !field.label.trim().is_empty())
        .map(|field| crate::api::mcps::slug_env_key(&field.label))
        .collect::<Vec<_>>();
    let digest = Sha256::digest(content.as_bytes());
    let bundle_id = format!(
        "legacy-{}",
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let payload = PluginBundlePayload {
        plugins: vec![PortablePluginConfig {
            source_config_id: "legacy-plugin".into(),
            label: spec.name.clone(),
            server,
            env_keys,
            values: None,
            args_override: None,
            was_global: false,
            include_general: true,
            preferred_interface: PluginInterface::default(),
        }],
    };
    let envelope = PluginBundleEnvelope {
        kind: PLUGIN_BUNDLE_KIND.into(),
        version: PLUGIN_BUNDLE_VERSION,
        bundle_id,
        exported_at: Utc::now(),
        includes_values: false,
        encrypted: false,
        plugin_labels: vec![spec.name],
        value_manifest: Vec::new(),
        payload: Some(payload.clone()),
        encrypted_payload: None,
        wrapped_key: None,
    };
    Ok((envelope, payload))
}

fn looks_secret(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "api-key",
        "api_key",
        "auth",
        "bearer",
        "credential",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

/// Arguments safe to show: `--flag=value` and the value after a secret-named
/// flag are masked, as is anything shaped like a credential.
fn mask_secret_args(args: &[String]) -> Vec<String> {
    const MASK: &str = "••••";
    let known_prefix = [
        "ghp_",
        "gho_",
        "github_pat_",
        "glpat-",
        "sk-",
        "xox",
        "AKIA",
        "eyJ",
    ];
    let mut previous_secret_flag = false;
    args.iter()
        .map(|arg| {
            let masked = if previous_secret_flag && !arg.starts_with('-') {
                MASK.to_string()
            } else if let Some((key, _)) = arg.split_once('=') {
                if looks_secret(key) || known_prefix.iter().any(|p| arg.contains(p)) {
                    format!("{key}={MASK}")
                } else {
                    arg.clone()
                }
            } else if known_prefix.iter().any(|prefix| arg.starts_with(prefix))
                || (arg.chars().count() >= 32
                    && arg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            {
                MASK.to_string()
            } else {
                arg.clone()
            };
            previous_secret_flag = arg.starts_with('-') && !arg.contains('=') && looks_secret(arg);
            masked
        })
        .collect()
}

fn build_import_preview(
    conn: &rusqlite::Connection,
    envelope: &PluginBundleEnvelope,
    payload: &PluginBundlePayload,
    fingerprint: &str,
    legacy: bool,
) -> anyhow::Result<ImportBundlePreview> {
    let already_imported = conn
        .query_row(
            "SELECT content_sha256 FROM plugin_bundle_imports WHERE source_bundle_id = ?1",
            [&envelope.bundle_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .is_some_and(|existing| existing == fingerprint);
    let servers = db::mcps::list_servers(conn)?;
    let configs = db::mcps::list_configs(conn)?;
    let plugins = payload
        .plugins
        .iter()
        .map(|portable| {
            let known = current_registry_server(&portable.server.id).or_else(|| {
                servers
                    .iter()
                    .find(|server| server.id == portable.server.id)
                    .cloned()
            });
            let (importable, issue) = if known.is_some() {
                (true, None)
            } else if has_reserved_registry_prefix(&portable.server.id) {
                (false, Some("reserved".to_string()))
            } else if !is_safe_manual_server(&portable.server) {
                (false, Some("untrusted_server".to_string()))
            } else {
                (true, None)
            };
            let own_args = known.as_ref().and_then(|server| match &server.transport {
                McpTransport::Stdio { args, .. } => Some(args.clone()),
                _ => None,
            });
            let proposed = portable.args_override.as_ref();
            let args_differ =
                own_args.is_some() && proposed.is_some() && proposed != own_args.as_ref();
            let exists = configs.iter().any(|config| {
                config.server_id == portable.server.id && config.label == portable.label
            });
            ImportPreviewPlugin {
                source_config_id: portable.source_config_id.clone(),
                label: portable.label.clone(),
                server_name: portable.server.name.clone(),
                usual_args: own_args.as_deref().map(mask_secret_args),
                proposed_args: if args_differ {
                    proposed.map(|args| mask_secret_args(args))
                } else {
                    None
                },
                args_differ,
                importable: importable && (!exists || already_imported),
                issue: issue
                    .or_else(|| (exists && !already_imported).then(|| "exists".to_string())),
            }
        })
        .collect();
    Ok(ImportBundlePreview {
        bundle_id: envelope.bundle_id.clone(),
        already_imported,
        includes_values: envelope.includes_values,
        legacy,
        plugins,
    })
}

/// POST /api/mcps/bundles/import-preview
pub async fn preview_plugin_bundle_import(
    State(state): State<AppState>,
    Json(request): Json<ImportPluginBundleRequest>,
) -> Json<ApiResponse<ImportBundlePreview>> {
    let (envelope, payload, fingerprint, legacy) =
        match load_bundle(&request.content, request.passphrase.as_deref()) {
            Ok(loaded) => loaded,
            Err((code, message)) => return Json(ApiResponse::err_coded(code, message)),
        };
    match state
        .db
        .with_read_conn(move |conn| {
            build_import_preview(conn, &envelope, &payload, &fingerprint, legacy)
        })
        .await
    {
        Ok(preview) => Json(ApiResponse::ok(preview)),
        Err(error) => Json(ApiResponse::err(error.to_string())),
    }
}

/// POST /api/mcps/bundles/import
pub async fn import_plugin_bundle(
    State(state): State<AppState>,
    Json(request): Json<ImportPluginBundleRequest>,
) -> Json<ApiResponse<ImportPluginBundleReport>> {
    let (envelope, payload, fingerprint, _) =
        match load_bundle(&request.content, request.passphrase.as_deref()) {
            Ok(loaded) => loaded,
            Err((code, message)) => return Json(ApiResponse::err_coded(code, message)),
        };
    let instance_secret = match state.config.read().await.encryption_secret.clone() {
        Some(secret) => secret,
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    let consent: HashSet<String> = request.accept_args_for.into_iter().collect();
    match state
        .db
        .with_conn(move |conn| {
            import_payload(
                conn,
                &envelope,
                payload,
                &instance_secret,
                &fingerprint,
                &consent,
            )
        })
        .await
    {
        Ok(report) => Json(ApiResponse::ok(report)),
        Err(error) if error.to_string().starts_with("IMPORT_CONFLICT:") => {
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                error.to_string().trim_start_matches("IMPORT_CONFLICT: "),
            ))
        }
        Err(error) => Json(ApiResponse::err(format!(
            "Plugin bundle import failed: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ApiConfigKey, ApiSpec, TokenInjection};

    fn test_server(id: &str, auth: ApiAuthKind) -> McpServer {
        McpServer {
            id: id.into(),
            name: "Portable API".into(),
            description: "test".into(),
            transport: McpTransport::ApiOnly,
            source: McpSource::Manual,
            api_spec: Some(ApiSpec {
                base_url: "https://example.test".into(),
                auth,
                endpoints: Vec::new(),
                docs_url: None,
                config_keys: vec![ApiConfigKey {
                    env_key: "SITE_ID".into(),
                    label: "Site".into(),
                    placeholder: String::new(),
                    description: String::new(),
                }],
                default_headers: vec![],
                test_endpoint: None,
            }),
        }
    }

    fn test_config(server_id: &str) -> McpConfig {
        McpConfig {
            id: "config-source".into(),
            server_id: server_id.into(),
            label: "Portable config".into(),
            env_keys: vec!["API_TOKEN".into(), "SITE_ID".into()],
            env_encrypted: String::new(),
            args_override: None,
            is_global: false,
            include_general: true,
            config_hash: "source-hash".into(),
            project_ids: Vec::new(),
            host_sync: HostSyncMode::None,
        }
    }

    #[test]
    fn preview_marks_auth_values_sensitive_and_config_values_plain() {
        let server = test_server(
            "custom-portable",
            ApiAuthKind::Bearer {
                env_key: "API_TOKEN".into(),
            },
        );
        let values = plugin_value_descriptors(&server, &test_config(&server.id));
        assert_eq!(values.len(), 2);
        assert!(
            values
                .iter()
                .find(|value| value.key == "API_TOKEN")
                .unwrap()
                .sensitive
        );
        assert!(
            !values
                .iter()
                .find(|value| value.key == "SITE_ID")
                .unwrap()
                .sensitive
        );
    }

    #[test]
    fn cli_credentials_are_never_exportable() {
        let server = test_server(
            "trusted-cli",
            ApiAuthKind::CliToken {
                command: "vendor".into(),
                args: vec!["token".into()],
                inject: TokenInjection::BearerHeader,
                fallback_env_key: Some("API_TOKEN".into()),
            },
        );
        let values = plugin_value_descriptors(&server, &test_config(&server.id));
        assert!(
            !values
                .iter()
                .find(|value| value.key == "API_TOKEN")
                .unwrap()
                .exportable
        );
        assert!(
            values
                .iter()
                .find(|value| value.key == "SITE_ID")
                .unwrap()
                .exportable
        );
    }

    #[test]
    fn encrypted_payload_requires_the_right_passphrase() {
        let payload = PluginBundlePayload {
            plugins: vec![PortablePluginConfig {
                source_config_id: "source".into(),
                server: test_server("custom-portable", ApiAuthKind::None),
                label: "Portable".into(),
                env_keys: vec!["SITE_ID".into()],
                values: Some(BTreeMap::from([("SITE_ID".into(), "eu".into())])),
                args_override: None,
                was_global: false,
                include_general: true,
                preferred_interface: PluginInterface::Api,
            }],
        };
        let key_hex = crypto::generate_secret();
        let key = crypto::parse_secret(&key_hex).unwrap();
        let encrypted_payload =
            crypto::encrypt(&serde_json::to_string(&payload).unwrap(), &key).unwrap();
        let wrapped_key =
            recovery::to_code(&recovery::wrap_key(&key_hex, "correct horse battery").unwrap());
        let envelope = PluginBundleEnvelope {
            kind: PLUGIN_BUNDLE_KIND.into(),
            version: 1,
            bundle_id: "bundle".into(),
            exported_at: Utc::now(),
            includes_values: true,
            encrypted: true,
            plugin_labels: vec!["Portable".into()],
            value_manifest: Vec::new(),
            payload: None,
            encrypted_payload: Some(encrypted_payload),
            wrapped_key: Some(wrapped_key),
        };
        assert!(decode_payload(&envelope, None).is_err());
        assert!(decode_payload(&envelope, Some("wrong passphrase")).is_err());
        assert_eq!(
            decode_payload(&envelope, Some("correct horse battery"))
                .unwrap()
                .plugins[0]
                .values
                .as_ref()
                .unwrap()
                .get("SITE_ID")
                .map(String::as_str),
            Some("eu")
        );
    }

    #[test]
    fn unknown_executable_servers_are_not_safe_to_import() {
        let mut server = test_server("unknown", ApiAuthKind::None);
        server.transport = McpTransport::Stdio {
            command: "untrusted".into(),
            args: vec![],
        };
        assert!(!is_safe_manual_server(&server));
    }

    #[tokio::test]
    async fn encrypted_bundle_round_trips_values_and_replays_idempotently() {
        let source = crate::db::Database::open_in_memory().unwrap();
        let target = crate::db::Database::open_in_memory().unwrap();
        let source_secret = crypto::generate_secret();
        let target_secret = crypto::generate_secret();
        let source_secret_for_seed = source_secret.clone();
        source
            .with_conn(move |conn| {
                let server = test_server(
                    "custom-portable-roundtrip",
                    ApiAuthKind::Bearer {
                        env_key: "API_TOKEN".into(),
                    },
                );
                db::mcps::upsert_server(conn, &server)?;
                let env = HashMap::from([
                    ("API_TOKEN".into(), "token-value".into()),
                    ("SITE_ID".into(), "fr".into()),
                    ("LD_PRELOAD".into(), "/tmp/untrusted.so".into()),
                ]);
                let encrypted = db::mcps::encrypt_env(&env, &source_secret_for_seed)
                    .map_err(anyhow::Error::msg)?;
                let hash = db::mcps::compute_config_hash(&server, &env, None);
                db::mcps::insert_config(
                    conn,
                    &McpConfig {
                        id: "source-config".into(),
                        server_id: server.id,
                        label: "Portable production".into(),
                        env_keys: vec!["API_TOKEN".into(), "SITE_ID".into(), "LD_PRELOAD".into()],
                        env_encrypted: encrypted,
                        args_override: None,
                        is_global: true,
                        include_general: true,
                        config_hash: hash,
                        project_ids: Vec::new(),
                        host_sync: HostSyncMode::MirrorAll,
                    },
                )?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let source_secret_for_export = source_secret.clone();
        let (envelope, _) = source
            .with_conn(move |conn| {
                build_export(
                    conn,
                    ExportPluginBundleRequest {
                        config_ids: vec!["source-config".into()],
                        include_values: true,
                        passphrase: Some("portable-passphrase".into()),
                        confirmation: Some(SECRET_CONFIRMATION.into()),
                    },
                    &source_secret_for_export,
                )
            })
            .await
            .unwrap();
        assert!(envelope.encrypted);
        assert!(envelope.payload.is_none());
        let payload = decode_payload(&envelope, Some("portable-passphrase")).unwrap();
        validate_payload_contract(&envelope, &payload).unwrap();
        let fingerprint = semantic_fingerprint(&envelope).unwrap();

        let target_secret_for_import = target_secret.clone();
        let envelope_for_import = envelope.clone();
        let report = target
            .with_conn(move |conn| {
                import_payload(
                    conn,
                    &envelope_for_import,
                    payload,
                    &target_secret_for_import,
                    &fingerprint,
                    &HashSet::new(),
                )
            })
            .await
            .unwrap();
        assert_eq!(report.imported_config_ids.len(), 1);
        assert_eq!(report.imported_configs.len(), 1);
        assert_eq!(
            report.imported_configs[0].config_id,
            report.imported_config_ids[0]
        );
        assert_eq!(report.imported_configs[0].label, "Portable production");
        assert_eq!(report.imported_configs[0].server_name, "Portable API");
        assert_eq!(report.skipped_plugins, 0);
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| conflict.contains("LD_PRELOAD")));
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("global")));

        let imported_id = report.imported_config_ids[0].clone();
        let target_secret_for_read = target_secret.clone();
        target
            .with_conn(move |conn| {
                let config = db::mcps::get_config(conn, &imported_id)?.unwrap();
                assert!(!config.is_global);
                assert_eq!(config.host_sync, HostSyncMode::None);
                let env = db::mcps::decrypt_env(&config.env_encrypted, &target_secret_for_read)
                    .map_err(anyhow::Error::msg)?;
                assert_eq!(
                    env.get("API_TOKEN").map(String::as_str),
                    Some("token-value")
                );
                assert_eq!(env.get("SITE_ID").map(String::as_str), Some("fr"));
                assert!(!env.contains_key("LD_PRELOAD"));
                let audit_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM plugin_bundle_events WHERE action = 'import'",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(audit_count, 1);
                Ok::<_, anyhow::Error>(())
            })
            .await
            .unwrap();

        let replay_payload = decode_payload(&envelope, Some("portable-passphrase")).unwrap();
        let replay_fingerprint = semantic_fingerprint(&envelope).unwrap();
        let replay_envelope = envelope.clone();
        let replay = target
            .with_conn(move |conn| {
                import_payload(
                    conn,
                    &replay_envelope,
                    replay_payload,
                    &target_secret,
                    &replay_fingerprint,
                    &HashSet::new(),
                )
            })
            .await
            .unwrap();
        assert!(replay.already_imported);
        assert_eq!(replay.imported_config_ids, report.imported_config_ids);

        let mut changed = envelope;
        changed.plugin_labels.push("changed".into());
        let changed_payload = decode_payload(&changed, Some("portable-passphrase")).unwrap();
        let changed_fingerprint = semantic_fingerprint(&changed).unwrap();
        let conflict = target
            .with_conn(move |conn| {
                import_payload(
                    conn,
                    &changed,
                    changed_payload,
                    &crypto::generate_secret(),
                    &changed_fingerprint,
                    &HashSet::new(),
                )
            })
            .await
            .unwrap_err();
        assert!(conflict.to_string().starts_with("IMPORT_CONFLICT:"));
    }

    fn clear_envelope(plugins: Vec<PortablePluginConfig>) -> PluginBundleEnvelope {
        PluginBundleEnvelope {
            kind: PLUGIN_BUNDLE_KIND.into(),
            version: 1,
            bundle_id: Uuid::new_v4().to_string(),
            exported_at: Utc::now(),
            includes_values: false,
            encrypted: false,
            plugin_labels: Vec::new(),
            value_manifest: Vec::new(),
            payload: Some(PluginBundlePayload { plugins }),
            encrypted_payload: None,
            wrapped_key: None,
        }
    }

    fn portable(server: McpServer, args_override: Option<Vec<String>>) -> PortablePluginConfig {
        PortablePluginConfig {
            source_config_id: Uuid::new_v4().to_string(),
            label: format!("{} bundle", server.id),
            server,
            env_keys: Vec::new(),
            values: None,
            args_override,
            was_global: false,
            include_general: true,
            preferred_interface: PluginInterface::Mcp,
        }
    }

    fn import_clear(
        conn: &rusqlite::Connection,
        plugins: Vec<PortablePluginConfig>,
        accept_args_override: bool,
    ) -> ImportPluginBundleReport {
        let consent = if accept_args_override {
            plugins.iter().map(|p| p.source_config_id.clone()).collect()
        } else {
            HashSet::new()
        };
        import_with_consent(conn, plugins, &consent)
    }

    fn import_with_consent(
        conn: &rusqlite::Connection,
        plugins: Vec<PortablePluginConfig>,
        consent: &HashSet<String>,
    ) -> ImportPluginBundleReport {
        let envelope = clear_envelope(plugins);
        let payload = envelope.payload.clone().unwrap();
        let fingerprint = semantic_fingerprint(&envelope).unwrap();
        import_payload(
            conn,
            &envelope,
            payload,
            &crypto::generate_secret(),
            &fingerprint,
            consent,
        )
        .unwrap()
    }

    #[test]
    fn bundled_args_override_on_a_registry_server_needs_explicit_consent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let github = current_registry_server("mcp-github").unwrap();
        let evil = vec!["-y".to_string(), "evil-package".to_string()];

        let report = import_clear(
            &conn,
            vec![portable(github.clone(), Some(evil.clone()))],
            false,
        );
        assert_eq!(report.imported_config_ids.len(), 1);
        assert!(report
            .conflicts
            .iter()
            .any(|c| c.contains("custom argument(s) were not applied")));
        let config = db::mcps::get_config(&conn, &report.imported_config_ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(config.args_override, None);

        let mut consented = portable(github, Some(evil.clone()));
        consented.label = "consented".into();
        let report = import_clear(&conn, vec![consented], true);
        let config = db::mcps::get_config(&conn, &report.imported_config_ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(config.args_override, Some(evil));
    }

    #[test]
    fn args_override_equal_to_the_registry_args_is_a_silent_no_op() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let github = current_registry_server("mcp-github").unwrap();
        let McpTransport::Stdio { args, .. } = github.transport.clone() else {
            panic!("mcp-github is a stdio server");
        };
        let report = import_clear(&conn, vec![portable(github, Some(args))], false);
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        let config = db::mcps::get_config(&conn, &report.imported_config_ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(config.args_override, None);
    }

    #[test]
    fn reserved_catalogue_ids_are_refused_for_manual_servers() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let report = import_clear(
            &conn,
            vec![
                portable(test_server("mcp-not-in-catalogue", ApiAuthKind::None), None),
                portable(test_server("api-not-in-catalogue", ApiAuthKind::None), None),
            ],
            false,
        );
        assert!(report.imported_config_ids.is_empty());
        assert_eq!(report.skipped_plugins, 2);
        assert!(report.conflicts.iter().all(|c| c.contains("reserved")));
        assert!(db::mcps::list_servers(&conn)
            .unwrap()
            .iter()
            .all(|server| !server.id.ends_with("-not-in-catalogue")));
    }

    #[test]
    fn args_override_travels_only_in_the_encrypted_payload() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let secret = crypto::generate_secret();
        let server = current_registry_server("mcp-github").unwrap();
        db::mcps::upsert_server(&conn, &server).unwrap();
        let env = HashMap::from([("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), String::new())]);
        let args = vec!["--token".to_string(), "abc-secret-arg".to_string()];
        db::mcps::insert_config(
            &conn,
            &McpConfig {
                id: "with-args".into(),
                server_id: server.id.clone(),
                label: "GitHub with args".into(),
                env_keys: vec!["GITHUB_PERSONAL_ACCESS_TOKEN".into()],
                env_encrypted: db::mcps::encrypt_env(&env, &secret).unwrap(),
                args_override: Some(args.clone()),
                is_global: false,
                include_general: true,
                config_hash: db::mcps::compute_config_hash(&server, &env, Some(&args)),
                project_ids: Vec::new(),
                host_sync: HostSyncMode::None,
            },
        )
        .unwrap();

        let (clear, _) = build_export(
            &conn,
            ExportPluginBundleRequest {
                config_ids: vec!["with-args".into()],
                include_values: false,
                passphrase: None,
                confirmation: None,
            },
            &secret,
        )
        .unwrap();
        assert!(!serde_json::to_string(&clear)
            .unwrap()
            .contains("abc-secret-arg"));
        assert_eq!(clear.payload.unwrap().plugins[0].args_override, None);

        let (encrypted, _) = build_export(
            &conn,
            ExportPluginBundleRequest {
                config_ids: vec!["with-args".into()],
                include_values: true,
                passphrase: Some("portable-passphrase".into()),
                confirmation: Some(SECRET_CONFIRMATION.into()),
            },
            &secret,
        )
        .unwrap();
        assert!(!serde_json::to_string(&encrypted)
            .unwrap()
            .contains("abc-secret-arg"));
        let payload = decode_payload(&encrypted, Some("portable-passphrase")).unwrap();
        assert_eq!(payload.plugins[0].args_override, Some(args));
    }

    #[test]
    fn consent_applies_only_to_the_checked_plugins() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let github = current_registry_server("mcp-github").unwrap();
        let args = vec!["-y".to_string(), "other-package".to_string()];
        let mut first = portable(github.clone(), Some(args.clone()));
        first.label = "first".into();
        let mut second = portable(github, Some(args.clone()));
        second.label = "second".into();
        let consent = HashSet::from([first.source_config_id.clone()]);

        let report = import_with_consent(&conn, vec![first, second], &consent);
        let by_label = |label: &str| {
            let id = &report
                .imported_configs
                .iter()
                .find(|item| item.label == label)
                .unwrap()
                .config_id;
            db::mcps::get_config(&conn, id).unwrap().unwrap()
        };
        assert_eq!(by_label("first").args_override, Some(args));
        assert_eq!(by_label("second").args_override, None);
        assert_eq!(
            report
                .conflicts
                .iter()
                .filter(|c| c.contains("were not applied"))
                .count(),
            1
        );
    }

    #[test]
    fn a_reimport_can_add_the_consent_the_first_import_lacked() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let github = current_registry_server("mcp-github").unwrap();
        let args = vec!["-y".to_string(), "other-package".to_string()];
        let plugin = portable(github, Some(args.clone()));
        let source_id = plugin.source_config_id.clone();
        let envelope = clear_envelope(vec![plugin]);
        let payload = envelope.payload.clone().unwrap();
        let fingerprint = semantic_fingerprint(&envelope).unwrap();
        let secret = crypto::generate_secret();

        let first = import_payload(
            &conn,
            &envelope,
            payload.clone(),
            &secret,
            &fingerprint,
            &HashSet::new(),
        )
        .unwrap();
        let config_id = first.imported_config_ids[0].clone();
        assert_eq!(
            db::mcps::get_config(&conn, &config_id)
                .unwrap()
                .unwrap()
                .args_override,
            None
        );

        let again = import_payload(
            &conn,
            &envelope,
            payload.clone(),
            &secret,
            &fingerprint,
            &HashSet::new(),
        )
        .unwrap();
        assert!(again.already_imported);
        assert_eq!(
            db::mcps::get_config(&conn, &config_id)
                .unwrap()
                .unwrap()
                .args_override,
            None
        );

        let consented = import_payload(
            &conn,
            &envelope,
            payload,
            &secret,
            &fingerprint,
            &HashSet::from([source_id]),
        )
        .unwrap();
        assert!(consented.already_imported);
        assert_eq!(consented.imported_config_ids, vec![config_id.clone()]);
        assert_eq!(
            db::mcps::get_config(&conn, &config_id)
                .unwrap()
                .unwrap()
                .args_override,
            Some(args)
        );
    }

    #[test]
    fn secret_looking_arguments_are_masked_for_display() {
        let masked = mask_secret_args(&[
            "-y".into(),
            "@scope/server".into(),
            "--token".into(),
            "abc123".into(),
            "--api-key=hunter2".into(),
            "ghp_0123456789abcdef".into(),
            "--region=eu-west-1".into(),
        ]);
        assert_eq!(
            masked,
            vec![
                "-y",
                "@scope/server",
                "--token",
                "••••",
                "--api-key=••••",
                "••••",
                "--region=eu-west-1"
            ]
        );
    }

    #[test]
    fn the_import_preview_shows_the_usual_and_the_proposed_command() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let github = current_registry_server("mcp-github").unwrap();
        let McpTransport::Stdio { args: usual, .. } = github.transport.clone() else {
            panic!("stdio");
        };
        let proposed = vec![
            "--token".to_string(),
            "s3cr3t".to_string(),
            "evil".to_string(),
        ];
        let same = portable(github.clone(), Some(usual.clone()));
        let mut different = portable(github, Some(proposed));
        different.label = "different".into();
        let envelope = clear_envelope(vec![same, different]);
        let payload = envelope.payload.clone().unwrap();
        let fingerprint = semantic_fingerprint(&envelope).unwrap();

        let preview =
            build_import_preview(&conn, &envelope, &payload, &fingerprint, false).unwrap();
        assert!(!preview.plugins[0].args_differ);
        assert_eq!(preview.plugins[0].proposed_args, None);
        let different = &preview.plugins[1];
        assert!(different.args_differ);
        assert_eq!(different.usual_args.as_ref().unwrap(), &usual);
        assert_eq!(
            different.proposed_args.as_ref().unwrap(),
            &vec![
                "--token".to_string(),
                "••••".to_string(),
                "evil".to_string()
            ]
        );
        assert!(!serde_json::to_string(&preview).unwrap().contains("s3cr3t"));
    }

    #[test]
    fn an_old_single_plugin_json_becomes_a_one_plugin_bundle() {
        let old = r#"{"name":"Legacy API","base_url":"https://api.legacy.test","description":"d","docs_url":null,
            "fields":[{"label":"API Key","value":"leaked-value"}],
            "endpoints":[{"path":"/things","method":"GET","description":"list"}],"auth":"None"}"#;
        let (envelope, payload, _, legacy) = load_bundle(old, None).unwrap();
        assert!(legacy);
        assert_eq!(envelope.kind, PLUGIN_BUNDLE_KIND);
        let plugin = &payload.plugins[0];
        assert_eq!(plugin.label, "Legacy API");
        assert_eq!(plugin.env_keys, vec!["API_KEY".to_string()]);
        assert!(plugin.values.is_none(), "credentials never travel");
        assert!(matches!(plugin.server.transport, McpTransport::ApiOnly));

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::migrations::run(&conn).unwrap();
        let fingerprint = semantic_fingerprint(&envelope).unwrap();
        let report = import_payload(
            &conn,
            &envelope,
            payload,
            &crypto::generate_secret(),
            &fingerprint,
            &HashSet::new(),
        )
        .unwrap();
        assert_eq!(report.imported_config_ids.len(), 1);
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
    }

    #[test]
    fn a_malformed_old_plugin_json_is_refused_with_a_clear_error() {
        let missing_url = r#"{"name":"Legacy","base_url":"  ","fields":[]}"#;
        let (code, message) = load_bundle(missing_url, None).unwrap_err();
        assert!(matches!(code, ApiErrorCode::Validation));
        assert!(message.contains("base_url"), "{message}");
        let not_json = load_bundle("{ nope", None).unwrap_err();
        assert!(not_json.1.contains("Invalid plugin bundle JSON"));
        let wrong_shape = load_bundle(r#"{"name": 5}"#, None).unwrap_err();
        assert!(
            wrong_shape.1.contains("Not a plugin bundle"),
            "{}",
            wrong_shape.1
        );
    }
}
