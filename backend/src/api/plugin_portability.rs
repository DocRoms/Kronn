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
pub struct ImportPluginBundleReport {
    pub bundle_id: String,
    pub already_imported: bool,
    pub imported_config_ids: Vec<String>,
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
            args_override: config.args_override.clone(),
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

fn import_payload(
    conn: &rusqlite::Connection,
    envelope: &PluginBundleEnvelope,
    payload: PluginBundlePayload,
    instance_secret: &str,
    fingerprint: &str,
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
        return Ok(report);
    }

    let transaction = conn.unchecked_transaction()?;
    let mut imported_config_ids = Vec::new();
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
                args_override: portable.args_override.clone(),
                is_global: false,
                include_general: portable.include_general,
                config_hash: String::new(),
                project_ids: Vec::new(),
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

        let hash = db::mcps::compute_config_hash(&server, &env, portable.args_override.as_ref());
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
        let config = McpConfig {
            id: config_id.clone(),
            server_id: server.id,
            label: portable.label.clone(),
            env_keys,
            env_encrypted: encrypted,
            args_override: portable.args_override,
            // Never broaden project/host exposure during import.
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
        imported_config_ids.push(config_id);
    }

    let report = ImportPluginBundleReport {
        bundle_id: envelope.bundle_id.clone(),
        already_imported: false,
        skipped_plugins,
        includes_values: envelope.includes_values,
        imported_config_ids: imported_config_ids.clone(),
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

/// POST /api/mcps/bundles/import
pub async fn import_plugin_bundle(
    State(state): State<AppState>,
    Json(request): Json<ImportPluginBundleRequest>,
) -> Json<ApiResponse<ImportPluginBundleReport>> {
    let envelope: PluginBundleEnvelope = match serde_json::from_str(&request.content) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!("Invalid plugin bundle JSON: {error}"),
            ))
        }
    };
    if envelope.kind != PLUGIN_BUNDLE_KIND || envelope.version != PLUGIN_BUNDLE_VERSION {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            format!(
                "Unsupported plugin bundle (kind `{}`, version {})",
                envelope.kind, envelope.version
            ),
        ));
    }
    if envelope.bundle_id.trim().is_empty() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Plugin bundle id is required",
        ));
    }
    let payload = match decode_payload(&envelope, request.passphrase.as_deref()) {
        Ok(payload) => payload,
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                error.to_string(),
            ))
        }
    };
    if let Err(error) = validate_payload_contract(&envelope, &payload) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        ));
    }
    let fingerprint = match semantic_fingerprint(&envelope) {
        Ok(fingerprint) => fingerprint,
        Err(error) => return Json(ApiResponse::err(error.to_string())),
    };
    let instance_secret = match state.config.read().await.encryption_secret.clone() {
        Some(secret) => secret,
        None => return Json(ApiResponse::err("No encryption secret configured")),
    };
    match state
        .db
        .with_conn(move |conn| {
            import_payload(conn, &envelope, payload, &instance_secret, &fingerprint)
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
                )
            })
            .await
            .unwrap();
        assert_eq!(report.imported_config_ids.len(), 1);
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
                )
            })
            .await
            .unwrap_err();
        assert!(conflict.to_string().starts_with("IMPORT_CONFLICT:"));
    }
}
