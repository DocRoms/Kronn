//! Provider keys and the API auth token, encrypted with the instance key in
//! `stored_credentials` instead of in plaintext in `config.toml` (KT-1007).
//!
//! The running process keeps reading `config.tokens.keys` and
//! `config.server.auth_token`: [`boot`] fills them from the table, and every
//! `config::save` first writes them back through [`sync_for_save`]. Until
//! [`boot`] has stored and read back every value ("armed"), saves keep writing
//! them to `config.toml` exactly as before, so an interrupted or failed
//! migration never loses a credential.
//!
//! Migration order, safe to interrupt at any step and rerun:
//! 1. upsert the merged set (table ∪ config.toml, the file wins on the same id);
//! 2. read every row back and compare the decrypted values;
//! 3. back up the old `config.toml`, encrypted with the instance key;
//! 4. arm, then save `config.toml` without the credentials;
//! 5. scrub the credentials from `config.toml.backup` (the copy the DB
//!    migration runner makes before it migrates).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};

use crate::core::{crypto, keystore::KeyOutcome};
use crate::db::stored_credentials::{
    self as rows, StoredCredential, AUTH_TOKEN_ID, KIND_AUTH_TOKEN, KIND_PROVIDER_KEY,
};
use crate::db::Database;
use crate::models::{ApiKey, AppConfig};

/// The pre-migration `config.toml`, encrypted with the instance key. The
/// recovery passphrase restores that key, so it also opens this backup.
pub const BACKUP_FILENAME: &str = "config.toml.pre-credential-store.enc";
/// Copy written by the DB migration runner before pending migrations.
const MIGRATION_BACKUP_FILENAME: &str = "config.toml.backup";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlainCredential {
    kind: String,
    id: String,
    name: String,
    provider: String,
    active: bool,
    value: String,
}

impl PlainCredential {
    fn row_key(&self) -> (String, String) {
        (self.kind.clone(), self.id.clone())
    }

    fn auth_token(value: String) -> Self {
        Self {
            kind: KIND_AUTH_TOKEN.into(),
            id: AUTH_TOKEN_ID.into(),
            name: String::new(),
            provider: String::new(),
            active: true,
            value,
        }
    }
}

/// Credentials the live config holds, provider keys in their display order.
fn from_config(config: &AppConfig) -> Vec<PlainCredential> {
    let mut out: Vec<PlainCredential> = config
        .tokens
        .keys
        .iter()
        .map(|k| PlainCredential {
            kind: KIND_PROVIDER_KEY.into(),
            id: k.id.clone(),
            name: k.name.clone(),
            provider: k.provider.clone(),
            active: k.active,
            value: k.value.clone(),
        })
        .collect();
    if let Some(token) = config
        .server
        .auth_token
        .clone()
        .filter(|t| !t.is_empty() && !config.server.auth_token_session_only)
    {
        // `active` stores whether auth is enabled, so a lost config.toml does
        // not turn auth off silently.
        out.push(PlainCredential {
            active: config.server.auth_enabled,
            ..PlainCredential::auth_token(token)
        });
    }
    out
}

fn apply_to_config(config: &mut AppConfig, creds: &[PlainCredential]) {
    config.tokens.keys = creds
        .iter()
        .filter(|c| c.kind == KIND_PROVIDER_KEY)
        .map(|c| ApiKey {
            id: c.id.clone(),
            name: c.name.clone(),
            provider: c.provider.clone(),
            value: c.value.clone(),
            active: c.active,
        })
        .collect();
    config.server.auth_token = creds
        .iter()
        .find(|c| c.kind == KIND_AUTH_TOKEN)
        .map(|c| c.value.clone());
    config.server.auth_token_session_only = false;
}

/// Whether an auth token row is stored (readable or not).
pub async fn stored_auth_token_exists(db: &Database) -> Result<bool> {
    Ok(db
        .with_conn(rows::list)
        .await?
        .iter()
        .any(|r| r.kind == KIND_AUTH_TOKEN))
}

/// Table rows first, then config.toml entries; the file wins on the same id
/// because it is only written while the store is not armed, so it is newer.
fn merge(table: &[PlainCredential], file: &[PlainCredential]) -> Vec<PlainCredential> {
    let mut merged: Vec<PlainCredential> = table.to_vec();
    for cred in file {
        match merged.iter_mut().find(|c| c.row_key() == cred.row_key()) {
            Some(slot) => *slot = cred.clone(),
            None => merged.push(cred.clone()),
        }
    }
    merged
}

fn encrypt_rows(creds: &[PlainCredential], key_hex: &str) -> Result<Vec<StoredCredential>> {
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    creds
        .iter()
        .enumerate()
        .map(|(position, c)| {
            Ok(StoredCredential {
                kind: c.kind.clone(),
                id: c.id.clone(),
                name: c.name.clone(),
                provider: c.provider.clone(),
                active: c.active,
                position: position as i64,
                value_encrypted: crypto::encrypt(&c.value, &key).map_err(anyhow::Error::msg)?,
            })
        })
        .collect()
}

fn decrypt_value(encrypted: &str, key_hex: &str) -> Result<String> {
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    crypto::decrypt(encrypted, &key).map_err(anyhow::Error::msg)
}

/// Write `creds` (replacing the table or merging into it), then read every row
/// back and require the exact decrypted values.
async fn write_and_verify(
    db: &Database,
    creds: &[PlainCredential],
    preserve: &HashSet<(String, String)>,
    replace: bool,
    key_hex: &str,
) -> Result<()> {
    let encrypted = encrypt_rows(creds, key_hex)?;
    let expected = creds.to_vec();
    let preserve = preserve.clone();
    let key = key_hex.to_string();
    db.with_conn(move |conn| {
        // Write and read back in one transaction: a mismatch commits nothing.
        let tx = conn.unchecked_transaction()?;
        if replace {
            rows::replace_rows(&tx, &encrypted, &preserve)?;
        } else {
            rows::upsert_rows(&tx, &encrypted)?;
        }
        let stored: HashMap<(String, String), StoredCredential> = rows::list(&tx)?
            .into_iter()
            .map(|r| (r.row_key(), r))
            .collect();
        for want in &expected {
            let row = stored.get(&want.row_key()).with_context(|| {
                format!("credential {}:{} missing after write", want.kind, want.id)
            })?;
            let value = decrypt_value(&row.value_encrypted, &key).with_context(|| {
                format!("credential {}:{} does not decrypt", want.kind, want.id)
            })?;
            if value != want.value
                || row.name != want.name
                || row.provider != want.provider
                || row.active != want.active
            {
                anyhow::bail!("credential {}:{} read back differently", want.kind, want.id);
            }
        }
        if replace {
            let wanted: HashSet<_> = expected.iter().map(PlainCredential::row_key).collect();
            if let Some(extra) = stored
                .keys()
                .find(|k| !wanted.contains(*k) && !preserve.contains(*k))
            {
                anyhow::bail!(
                    "credential {}:{} still stored after removal",
                    extra.0,
                    extra.1
                );
            }
        }
        tx.commit()?;
        Ok(())
    })
    .await
}

/// Why credentials cannot be changed while the store is not armed.
pub const LOCKED_MESSAGE: &str = "The encryption key is locked: provider keys and the API token \
     cannot be stored until it is restored (Settings → Recovery)";

/// Per data directory, the credentials config.toml held when the boot found
/// the store unarmable. Absent: no boot ran (tests), so saves stay as before.
static FILE_AT_BOOT: LazyLock<std::sync::Mutex<HashMap<PathBuf, Vec<PlainCredential>>>> =
    LazyLock::new(Default::default);

fn file_credentials_at_boot(dir: &Path) -> Option<Vec<PlainCredential>> {
    FILE_AT_BOOT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(dir)
        .cloned()
}

fn note_file_credentials(dir: &Path, config: &AppConfig) {
    FILE_AT_BOOT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(dir.to_path_buf(), from_config(config));
}

/// `Err(LOCKED_MESSAGE)` when a boot ran and the store is not armed: callers
/// about to add or change a credential refuse before touching the config.
pub fn refuse_if_locked(dir: &Path) -> std::result::Result<(), String> {
    if !is_armed(dir) && file_credentials_at_boot(dir).is_some() {
        return Err(locked_message(dir));
    }
    Ok(())
}

/// Why credentials cannot be stored now: the key is locked, or it is in use
/// but the credential store failed to load at start (say so, and what to do).
fn locked_message(dir: &Path) -> String {
    match boot_failure(dir) {
        Some(failure) => format!(
            "The stored credentials could not be loaded at start ({failure}). Nothing was \
             lost: fix the cause (free disk space, repair config.toml), then restart Kronn."
        ),
        None => LOCKED_MESSAGE.to_string(),
    }
}

/// Per data directory, why the last credential boot failed with the key in use.
static BOOT_FAILURES: LazyLock<std::sync::Mutex<HashMap<PathBuf, String>>> =
    LazyLock::new(Default::default);

/// Remember why the credential boot of `dir` failed (or clear it).
pub fn record_boot_failure(dir: &Path, failure: Option<String>) {
    let mut map = BOOT_FAILURES.lock().unwrap_or_else(|p| p.into_inner());
    match failure {
        Some(f) => map.insert(dir.to_path_buf(), f),
        None => map.remove(dir),
    };
}

/// Why the credential boot of `dir` failed with the key in use, if it did.
pub fn boot_failure(dir: &Path) -> Option<String> {
    BOOT_FAILURES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(dir)
        .cloned()
}

/// [`refuse_if_locked`] for the current data directory.
pub fn refuse_credential_change() -> std::result::Result<(), String> {
    match crate::core::config::config_dir() {
        Ok(dir) => refuse_if_locked(&dir),
        Err(_) => Ok(()),
    }
}

struct Armed {
    db: Arc<Database>,
    /// Rows the current key cannot decrypt: never rewritten nor deleted, unless
    /// the user explicitly sets a new value for that row (a new auth token).
    preserve: std::sync::Mutex<HashSet<(String, String)>>,
    /// What the table holds, as last written and read back.
    persisted: tokio::sync::Mutex<Vec<PlainCredential>>,
}

static ARMED: LazyLock<std::sync::Mutex<HashMap<PathBuf, Arc<Armed>>>> =
    LazyLock::new(Default::default);

fn armed_map() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Arc<Armed>>> {
    ARMED.lock().unwrap_or_else(|p| p.into_inner())
}

fn armed_for(dir: &Path) -> Option<Arc<Armed>> {
    armed_map().get(dir).cloned()
}

/// Whether credentials of `dir` live in the encrypted store.
pub fn is_armed(dir: &Path) -> bool {
    armed_for(dir).is_some()
}

/// Stop routing `dir`'s credentials to the store (tests, data-dir switch).
pub fn disarm(dir: &Path) {
    armed_map().remove(dir);
    FILE_AT_BOOT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(dir);
}

/// Called by `config::save` before it writes `config.toml`. Returns `true`
/// when the credentials are stored (encrypted, read back), so the file must
/// leave them out; `false` when the store is not armed for `dir`.
pub(crate) async fn sync_for_save(dir: &Path, config: &AppConfig) -> Result<bool> {
    let Some(armed) = armed_for(dir) else {
        // Not armed after a boot: config.toml may keep only what it already
        // held; a credential it did not hold is refused, never written in clear.
        if let Some(allowed) = file_credentials_at_boot(dir) {
            let new_one = from_config(config).into_iter().find(|c| {
                !allowed
                    .iter()
                    .any(|a| a.kind == c.kind && a.id == c.id && a.value == c.value)
            });
            if let Some(c) = new_one {
                anyhow::bail!(
                    "{} (refused to write {}:{} to config.toml in plaintext)",
                    locked_message(dir),
                    c.kind,
                    c.id
                );
            }
        }
        return Ok(false);
    };
    let desired = from_config(config);
    let mut persisted = armed.persisted.lock().await;
    if *persisted == desired {
        return Ok(true);
    }
    let key = config
        .encryption_secret
        .as_deref()
        .filter(|k| !k.is_empty())
        .context("the encryption key is locked: credentials cannot be stored")?;
    let mut preserve = armed
        .preserve
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    // A value set now for an undecryptable row (regenerated token, network
    // exposure) is an explicit replacement; every other such row stays as is.
    let replaced: Vec<(String, String)> = desired
        .iter()
        .map(PlainCredential::row_key)
        .filter(|k| preserve.contains(k))
        .collect();
    for key in &replaced {
        tracing::warn!(
            "credentials: replacing the undecryptable stored {}:{} with the new value",
            key.0,
            key.1
        );
        preserve.remove(key);
    }
    write_and_verify(&armed.db, &desired, &preserve, true, key).await?;
    *armed.preserve.lock().unwrap_or_else(|p| p.into_inner()) = preserve;
    *persisted = desired;
    Ok(true)
}

/// Stored rows the key in use cannot decrypt (0 when not armed).
pub fn locked_credential_count(dir: &Path) -> usize {
    armed_for(dir)
        .map(|a| a.preserve.lock().unwrap_or_else(|p| p.into_inner()).len())
        .unwrap_or(0)
}

/// Delete every stored credential of `dir` (factory reset).
pub async fn forget_all(dir: &Path, db: &Database) -> Result<()> {
    db.with_conn(rows::delete_all).await?;
    if let Some(armed) = armed_for(dir) {
        armed.persisted.lock().await.clear();
    }
    Ok(())
}

/// Delete every stored provider key of `dir`, keeping the API auth token: a
/// reset must not leave a LAN-bound instance without auth.
pub async fn forget_provider_keys(dir: &Path, db: &Database) -> Result<()> {
    db.with_conn(|conn| {
        Ok(conn.execute(
            "DELETE FROM stored_credentials WHERE kind = ?1",
            [KIND_PROVIDER_KEY],
        )?)
    })
    .await?;
    if let Some(armed) = armed_for(dir) {
        armed
            .persisted
            .lock()
            .await
            .retain(|c| c.kind != KIND_PROVIDER_KEY);
        armed
            .preserve
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(kind, _)| kind != KIND_PROVIDER_KEY);
    }
    Ok(())
}

/// Why [`boot`] runs: at startup, or after the key was restored at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootMode {
    Startup,
    Restore,
}

/// What [`boot`] did, for the boot log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialBoot {
    /// Credentials found in config.toml and now stored encrypted.
    pub moved_from_config: usize,
    pub generated_auth_token: bool,
    pub backup: Option<PathBuf>,
    /// Rows the current key cannot decrypt, left untouched.
    pub locked_rows: usize,
}

/// Load the stored credentials into `config`, move any still in config.toml
/// into the store, and arm it for `dir` (which must be `config::config_dir()`,
/// since the final save goes there). `Ok(None)` when the key is locked: the
/// store stays disarmed and config.toml keeps whatever it holds.
pub async fn boot(
    config: &mut AppConfig,
    db: Arc<Database>,
    dir: &Path,
    key_outcome: &KeyOutcome,
    env_auth_token: Option<&str>,
    mode: BootMode,
) -> Result<Option<CredentialBoot>> {
    // Until armed below, saves may only keep what config.toml holds now.
    note_file_credentials(dir, config);
    if matches!(key_outcome, KeyOutcome::Locked { .. }) {
        // A stored token we cannot read is "auth locked", never "no auth"; its
        // row still says whether auth was enabled.
        if config.server.auth_token.is_none() {
            let stored = db.with_conn(rows::list).await?;
            if let Some(row) = stored.iter().find(|r| r.kind == KIND_AUTH_TOKEN) {
                config.server.auth_locked = true;
                config.server.auth_enabled |= row.active;
            }
        }
        tracing::warn!(
            "credentials: the encryption key is locked — stored provider keys and the auth token \
             are unavailable until it is restored; config.toml is left as it is"
        );
        return Ok(None);
    }
    let Some(key) = config.encryption_secret.clone().filter(|k| !k.is_empty()) else {
        return Ok(None);
    };

    let stored = db.with_conn(rows::list).await?;
    // The stored token row remembers that auth was enabled (config.toml lost).
    if stored.iter().any(|r| r.kind == KIND_AUTH_TOKEN && r.active) {
        config.server.auth_enabled = true;
    }
    let mut table = Vec::new();
    let mut preserve = HashSet::new();
    for row in stored {
        match decrypt_value(&row.value_encrypted, &key) {
            Ok(value) => table.push(PlainCredential {
                kind: row.kind,
                id: row.id,
                name: row.name,
                provider: row.provider,
                active: row.active,
                value,
            }),
            Err(_) => {
                preserve.insert((row.kind, row.id));
            }
        }
    }
    // Same order as `from_config` (provider keys by position, then the token),
    // so an unchanged set compares equal and is never rewritten.
    table.sort_by_key(|c| c.kind != KIND_PROVIDER_KEY);
    if !preserve.is_empty() {
        tracing::error!(
            "credentials: {} stored credential(s) do not decrypt with the current key; they are \
             kept untouched and must be re-entered or restored with their key",
            preserve.len()
        );
    }

    let file = from_config(config);
    let moved_from_config = file.iter().filter(|c| !table.contains(c)).count();
    let mut merged = merge(&table, &file);

    let auth_key = (KIND_AUTH_TOKEN.to_string(), AUTH_TOKEN_ID.to_string());
    let mut generated_auth_token = false;
    let has_token =
        merged.iter().any(|c| c.kind == KIND_AUTH_TOKEN) || preserve.contains(&auth_key);
    if let (Some(token), false) = (env_auth_token.filter(|t| !t.is_empty()), has_token) {
        // The operator's KRONN_AUTH_TOKEN becomes the stored token.
        merged.push(PlainCredential::auth_token(token.to_string()));
    } else if !has_token {
        // First launch: auth defaults on natively, off under Docker (see
        // `core::env::auth_on_by_default`).
        merged.push(PlainCredential::auth_token(
            uuid::Uuid::new_v4().to_string(),
        ));
        // A restore never changes whether auth is on, and a start never
        // turns off an auth already on (a token row lost after a kill).
        if mode == BootMode::Startup {
            config.server.auth_enabled =
                config.server.auth_enabled || crate::core::env::auth_on_by_default();
        }
        generated_auth_token = true;
        tracing::info!(
            "Generated auth token (auth_enabled={}, docker={})",
            config.server.auth_enabled,
            crate::core::env::is_docker(),
        );
    }

    let mut canonical = config.clone();
    apply_to_config(&mut canonical, &merged);
    let merged = from_config(&canonical);
    if merged != table {
        write_and_verify(&db, &merged, &preserve, false, &key)
            .await
            .context("storing credentials in the encrypted table failed; config.toml unchanged")?;
    }

    let backup = back_up_config_if_it_holds_secrets(dir, &key)
        .context("backing up config.toml before removing its secrets failed")?;

    apply_to_config(config, &merged);
    config.server.auth_locked = config.server.auth_token.is_none() && preserve.contains(&auth_key);
    armed_map().insert(
        dir.to_path_buf(),
        Arc::new(Armed {
            db,
            preserve: std::sync::Mutex::new(preserve.clone()),
            persisted: tokio::sync::Mutex::new(merged),
        }),
    );
    crate::core::config::save(config).await?;
    scrub_migration_backup(dir, &key)?;

    if moved_from_config > 0 {
        tracing::info!(
            "credentials: {moved_from_config} credential(s) moved from config.toml to the encrypted \
             store{}",
            backup
                .as_ref()
                .map(|p| format!("; the previous file is kept encrypted at {}", p.display()))
                .unwrap_or_default()
        );
    }
    Ok(Some(CredentialBoot {
        moved_from_config,
        generated_auth_token,
        backup,
        locked_rows: preserve.len(),
    }))
}

/// Whether a config.toml text carries a key, token or credential value.
fn toml_holds_secrets(text: &str) -> bool {
    let key = text
        .parse::<toml::Table>()
        .ok()
        .and_then(|t| {
            t.get("encryption_secret")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
        })
        .unwrap_or(false);
    key || toml_holds_credentials(text)
}

/// Whether a config.toml text carries a token or credential value (the key
/// itself aside: a backup encrypted under that key would protect nothing).
pub(crate) fn toml_holds_credentials(text: &str) -> bool {
    let Ok(table) = text.parse::<toml::Table>() else {
        // Unparseable: treat as sensitive rather than skip the backup.
        return true;
    };
    let non_empty =
        |v: Option<&toml::Value>| v.and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty());
    if let Some(server) = table.get("server").and_then(|v| v.as_table()) {
        if non_empty(server.get("auth_token")) {
            return true;
        }
    }
    if let Some(tokens) = table.get("tokens").and_then(|v| v.as_table()) {
        if ["anthropic", "openai", "google"]
            .iter()
            .any(|k| non_empty(tokens.get(*k)))
        {
            return true;
        }
        if let Some(keys) = tokens.get("keys").and_then(|v| v.as_array()) {
            if keys
                .iter()
                .any(|k| non_empty(k.as_table().and_then(|t| t.get("value"))))
            {
                return true;
            }
        }
    }
    false
}

/// Encrypt the current config.toml to [`BACKUP_FILENAME`] (0600) when it still
/// holds secrets. An existing backup is kept: it is the oldest original.
pub(crate) fn back_up_config_if_it_holds_secrets(
    dir: &Path,
    key_hex: &str,
) -> Result<Option<PathBuf>> {
    let backup = dir.join(BACKUP_FILENAME);
    if backup.exists() {
        return Ok(Some(backup));
    }
    let text = match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("read config.toml"),
    };
    if !toml_holds_credentials(&text) {
        return Ok(None);
    }
    let key = crypto::parse_secret(key_hex).map_err(anyhow::Error::msg)?;
    let encrypted = crypto::encrypt(&text, &key).map_err(anyhow::Error::msg)?;
    let tmp = dir.join(format!(".{BACKUP_FILENAME}.tmp"));
    crate::core::keyvault::write_private_atomic(&tmp, &backup, encrypted.as_bytes())
        .context("write config backup")?;
    Ok(Some(backup))
}

/// Move the encrypted config.toml backup from `from_hex` to `to_hex` when a key
/// is retired; untouched when it is under another key or absent.
pub fn reencrypt_backup(dir: &Path, from_hex: &str, to_hex: &str) -> Result<bool> {
    let path = dir.join(BACKUP_FILENAME);
    let Ok(text) = read_backup(&path, from_hex) else {
        return Ok(false);
    };
    let to = crypto::parse_secret(to_hex).map_err(anyhow::Error::msg)?;
    let encrypted = crypto::encrypt(&text, &to).map_err(anyhow::Error::msg)?;
    let tmp = dir.join(format!(".{BACKUP_FILENAME}.reencrypt.tmp"));
    crate::core::keyvault::write_private_atomic(&tmp, &path, encrypted.as_bytes())
        .context("write config backup")?;
    Ok(true)
}

/// Make the encrypted config.toml backup readable with `key_hex`: when it is
/// under another candidate (a retirement that stopped half-way, an older key),
/// re-encrypt it. Untouched when absent, already current, or under no
/// candidate.
pub fn reencrypt_backup_to(
    dir: &Path,
    candidates: &[(String, &'static str)],
    key_hex: &str,
) -> Result<bool> {
    let path = dir.join(BACKUP_FILENAME);
    if !path.exists() || read_backup(&path, key_hex).is_ok() {
        return Ok(false);
    }
    for (from, _) in candidates {
        if from != key_hex && read_backup(&path, from).is_ok() {
            return reencrypt_backup(dir, from, key_hex);
        }
    }
    Ok(false)
}

/// Decrypt a backup written by [`back_up_config_if_it_holds_secrets`].
pub fn read_backup(path: &Path, key_hex: &str) -> Result<String> {
    let encrypted = std::fs::read_to_string(path).context("read config backup")?;
    decrypt_value(encrypted.trim(), key_hex)
}

/// `text` with the auth token and every provider key removed, for the copy the
/// DB migration runner keeps: written scrubbed from the start, so no failure
/// later can leave credentials in it. `None` when `text` is not valid TOML.
pub fn without_credentials(text: &str) -> Option<String> {
    let mut table = text.parse::<toml::Table>().ok()?;
    strip_credential_fields(&mut table);
    toml::to_string_pretty(&table).ok()
}

/// Remove the auth token and provider keys from a parsed config.toml.
fn strip_credential_fields(table: &mut toml::Table) -> bool {
    let mut changed = false;
    if let Some(server) = table.get_mut("server").and_then(|v| v.as_table_mut()) {
        changed |= server.remove("auth_token").is_some();
    }
    if let Some(tokens) = table.get_mut("tokens").and_then(|v| v.as_table_mut()) {
        for legacy in ["anthropic", "openai", "google"] {
            changed |= tokens.remove(legacy).is_some();
        }
        changed |= tokens.remove("keys").is_some();
    }
    changed
}

/// Remove credentials from the DB migration runner's plaintext copy. Its
/// `encryption_secret` goes only when it is the key in use and config.toml no
/// longer needs one; any other key is kept, since rows may depend on it.
fn scrub_migration_backup(dir: &Path, key_hex: &str) -> Result<()> {
    // Every file is attempted: one unreadable copy must not leave the others
    // with their credentials in clear. The failures are reported together.
    let errors: Vec<String> = migration_backup_paths(dir)
        .iter()
        .filter_map(|path| scrub_one_backup(dir, path, key_hex).err())
        .map(|e| format!("{e:#}"))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("could not scrub every config backup: {}", errors.join("; "))
    }
}

/// config.toml.backup and every rotated `config.toml.backup.<ts>` copy (those
/// rotated by an earlier boot may still hold 0.14.2 credentials in clear).
fn migration_backup_paths(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name == MIGRATION_BACKUP_FILENAME
                || name.starts_with(&format!("{MIGRATION_BACKUP_FILENAME}."))
        })
        .map(|e| e.path())
        .collect()
}

pub(crate) fn scrub_one_backup(dir: &Path, path: &Path, key_hex: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    if !toml_holds_secrets(&text) {
        return Ok(());
    }
    let Ok(mut table) = text.parse::<toml::Table>() else {
        return Ok(());
    };
    let mut changed = strip_credential_fields(&mut table);
    // Its key goes only when it is the key in use and config.toml no longer
    // needs a copy (the two-copies rule); any other key is kept.
    let same_key = table
        .get("encryption_secret")
        .and_then(|v| v.as_str())
        .is_some_and(|k| crate::core::keyvault::same_key(k, key_hex));
    if same_key && crate::core::config::retained_disk_key(dir).is_none() {
        changed |= table.remove("encryption_secret").is_some();
    }
    if !changed {
        return Ok(());
    }
    let content = toml::to_string_pretty(&table).context("serialize config backup")?;
    let file = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{file}.scrub.tmp"));
    crate::core::keyvault::write_private_atomic(&tmp, path, content.as_bytes())
        .with_context(|| format!("write {file}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "credential_store_tests.rs"]
mod tests;
