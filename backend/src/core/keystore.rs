//! DB-aware encryption-key reconciler — the single authority that decides which
//! key Kronn uses, run once at boot *after* the database is open.
//!
//! `config::load()` deliberately never mints a key (it runs before the DB is
//! open and can't tell whether encrypted data exists). This module does: it
//! gathers candidate keys from the [`KeyStore`] ladder (env → keychain →
//! sidecar) plus the legacy `config.toml` field, then decides by **decrypt
//! self-test** — the authority is "does this key actually decrypt an existing
//! `env_encrypted` row?", not where it came from.
//!
//! Invariants enforced here (the 2026-06-30 incident fixes):
//! - **I1** never generate a key when encrypted rows exist and none of the
//!   candidates decrypt them — that silent regeneration orphaned every secret.
//! - **I3** fail-soft: an unresolvable key locks the *token subsystem*, it never
//!   bricks boot and never rewrites/overwrites ciphertext.
//! - Mint only on a genuinely empty install; adopt an existing key otherwise.
//!   "Empty" covers every column of [`ENCRYPTED_COLUMNS`], not only MCP configs.
//! - A vault that cannot be read stops the boot ([`KeyBootError`]): it may hold
//!   the key, so neither minting nor mirroring over it is safe.
//! - Mirror the resolved key into every EMPTY writable vault so it survives a
//!   `config.toml` rewrite / keychain reset. The `config.toml` copy is dropped
//!   only when two independent copies remain (two vaults, or one plus a
//!   verified recovery passphrase) and the key decrypts every column; another
//!   value found in `config.toml` goes to `config.toml.retired-key.<ts>`.

use std::path::Path;

use anyhow::{Context, Result};

use crate::core::{config, crypto, keyvault::KeyStore, recovery};
use crate::db::Database;
use crate::models::AppConfig;

/// A database column holding ciphertext under the instance key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptedColumn {
    pub table: &'static str,
    pub column: &'static str,
}

/// Every column encrypted with the instance key. The reconciler reads all of
/// them before accepting or minting a key; a schema test fails when a new
/// encrypted column is missing here.
pub const ENCRYPTED_COLUMNS: &[EncryptedColumn] = &[
    EncryptedColumn {
        table: "mcp_configs",
        column: "env_encrypted",
    },
    EncryptedColumn {
        table: "execution_variable_snapshots",
        column: "values_encrypted",
    },
    EncryptedColumn {
        table: "stored_credentials",
        column: "value_encrypted",
    },
    EncryptedColumn {
        table: "project_github_connections",
        column: "token_encrypted",
    },
];

/// Ciphertext found in one registered column.
#[derive(Debug, Clone)]
struct ColumnRows {
    column: EncryptedColumn,
    total: usize,
    sample: Vec<String>,
}

/// Why the boot must stop. Each message says what to do; nothing was changed.
#[derive(Debug, thiserror::Error)]
pub enum KeyBootError {
    #[error(
        "Kronn cannot read its encryption key from the {vault} ({error}). Nothing was changed. \
         {hint} Then restart Kronn."
    )]
    VaultUnreadable {
        vault: &'static str,
        error: String,
        hint: &'static str,
    },
    #[error(
        "Several encryption keys each decrypt part of Kronn's encrypted data: {details}. \
         Nothing was changed: Kronn will not pick one, the other's data would be lost. To keep \
         everything, start Kronn with KRONN_REENCRYPT_FROM=<fingerprint of a key to retire>: \
         its rows are re-encrypted under the first other key listed, checked, and no key is \
         deleted; with more than two keys, repeat once per key to retire. macOS app, from a \
         terminal: open --env KRONN_REENCRYPT_FROM=<fingerprint> -a Kronn. Keep every copy \
         (the OS keychain item {service}/{account}, the encryption_key file in the data \
         directory, KRONN_ENCRYPTION_KEK, config.toml, config.toml.backup*, \
         config.toml.retired-key.*, config.toml.corrupt.*) until then."
    )]
    SeveralKeys {
        details: String,
        service: &'static str,
        account: &'static str,
    },
    #[error(
        "No durable copy of the encryption key could be written (no key store took it and \
         config.toml could not be written). Nothing was encrypted. Free disk space or make the \
         Kronn data directory writable, then restart Kronn."
    )]
    NoDurableCopy,
}

fn vault_hint(vault: &str, corrupted: bool) -> &'static str {
    match vault {
        "sidecar" if corrupted => {
            "The encryption_key file in the Kronn data directory is corrupted: move it aside \
             (keep the copy), then restart; the key is then read from the keychain or \
             config.toml, or restored with your recovery passphrase."
        }
        "keychain" if corrupted => {
            "The keychain item com.kronn.kronn / encryption_secret_v1 is damaged. If the \
             encryption_key file in the Kronn data directory or config.toml holds the key, delete \
             that item in Keychain Access (Kronn writes it back at the next start); otherwise \
             restore the key with your recovery passphrase. To start meanwhile without the \
             keychain: open --env KRONN_USE_KEYCHAIN=0 -a Kronn."
        }
        "keychain" => {
            "Unlock the OS keychain and choose Allow when it asks about Kronn. To use the copy \
             in the Kronn data directory instead, start Kronn with KRONN_USE_KEYCHAIN=0 (macOS \
             app, from a terminal: open --env KRONN_USE_KEYCHAIN=0 -a Kronn)."
        }
        "sidecar" => {
            "Make the encryption_key file in the Kronn data directory readable by your account \
             (owner-only, 0600)."
        }
        _ => "Make that key store readable.",
    }
}

fn decrypts(encrypted: &str, key_hex: &str) -> bool {
    crypto::parse_secret(key_hex)
        .and_then(|key| crypto::decrypt(encrypted, &key))
        .is_ok()
}

/// What the reconciler did — surfaced for logging and (later) the UI banner.
#[derive(Debug, Clone, PartialEq)]
pub enum KeyOutcome {
    /// Genuinely empty install → a fresh key was generated.
    Minted,
    /// An existing key was adopted (empty DB) or accepted (it decrypts the data).
    Resolved { source: &'static str },
    /// Encrypted rows exist but NO available key decrypts them. Fail-soft: boot
    /// continues, the token subsystem is locked, nothing was overwritten.
    Locked { encrypted_rows: usize },
}

/// The pure decision core — no DB, no I/O, fully unit-testable. Given candidate
/// keys (priority-ordered) and the existing ciphertext rows, decide what to do.
#[derive(Debug)]
enum Decision {
    /// Empty DB, no candidate anywhere → mint a fresh key.
    Mint,
    /// Empty DB, a candidate exists → adopt it (no data to validate against).
    Adopt(String, &'static str),
    /// Rows exist and this candidate decrypts at least one → accept it.
    Accept(String, &'static str),
    /// Rows exist and nothing decrypts them → lock (fail-soft).
    Lock(usize),
    /// Several distinct keys each decrypt some rows: stop, write nothing.
    /// (key, source, key fingerprint, rows it decrypts) per decrypting candidate.
    Conflict(Vec<(String, &'static str, String, usize)>),
}

fn decide(candidates: &[(String, &'static str)], columns: &[ColumnRows]) -> Decision {
    if columns.is_empty() {
        return match candidates.first() {
            Some((k, s)) => Decision::Adopt(k.clone(), s),
            None => Decision::Mint,
        };
    }
    // Authority = decrypt self-test, never provenance. Exactly one distinct key
    // may decrypt data: with two, choosing either would orphan the other's rows.
    let decrypting: Vec<(&String, &'static str, usize)> = candidates
        .iter()
        .map(|(cand, src)| {
            let n = columns
                .iter()
                .map(|col| col.sample.iter().filter(|enc| decrypts(enc, cand)).count())
                .sum::<usize>();
            (cand, *src, n)
        })
        .filter(|(_, _, n)| *n > 0)
        .collect();
    match decrypting.as_slice() {
        [] => Decision::Lock(columns.iter().map(|c| c.total).sum()),
        [(cand, src, _)] => Decision::Accept((*cand).clone(), src),
        several => Decision::Conflict(
            several
                .iter()
                .map(|(cand, src, n)| {
                    let fp = crypto::key_fingerprint_hex(cand).unwrap_or_else(|_| "?".into());
                    ((*cand).clone(), *src, fp, *n)
                })
                .collect(),
        ),
    }
}

/// Env var naming (by fingerprint) the key whose rows the next boot moves under
/// the other decrypting key, to resolve [`KeyBootError::SeveralKeys`].
pub const ENV_REENCRYPT_FROM: &str = "KRONN_REENCRYPT_FROM";

/// (from, to) keys when `KRONN_REENCRYPT_FROM` names one of the decrypting
/// keys: its rows go to the highest-priority other one. With more than two
/// keys, one key is retired per start.
fn reencrypt_request(found: &[(String, &'static str, String, usize)]) -> Option<(String, String)> {
    let wanted = crate::core::child_env::var(ENV_REENCRYPT_FROM).ok()?;
    let wanted = wanted.trim();
    if found.len() < 2 || wanted.is_empty() {
        return None;
    }
    let from = found
        .iter()
        .position(|(_, _, fp, _)| fp.eq_ignore_ascii_case(wanted))?;
    let to = found.iter().position(|(key, ..)| *key != found[from].0)?;
    Some((found[from].0.clone(), found[to].0.clone()))
}

/// Rows of the registered columns that `key` does not decrypt (kept untouched;
/// "Re-encrypt imported secrets" may move them).
pub async fn undecryptable_rows(db: &Database, key: &str) -> Result<usize> {
    Ok(collect_encrypted_rows(db)
        .await?
        .iter()
        .map(|col| col.sample.iter().filter(|enc| !decrypts(enc, key)).count())
        .sum())
}

/// True when `key` decrypts at least one sampled row of every non-empty column.
fn decrypts_every_column(key: &str, columns: &[ColumnRows]) -> bool {
    columns
        .iter()
        .all(|col| col.sample.iter().any(|enc| decrypts(enc, key)))
}

/// Read the ciphertext of every registered column; empty columns are omitted.
async fn collect_encrypted_rows(db: &Database) -> Result<Vec<ColumnRows>> {
    db.with_conn(|conn| {
        let mut out = Vec::new();
        for col in ENCRYPTED_COLUMNS {
            let filter = format!("{c} IS NOT NULL AND {c} != ''", c = col.column);
            let total: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM {} WHERE {filter}", col.table),
                [],
                |row| row.get(0),
            )?;
            if total == 0 {
                continue;
            }
            let mut stmt = conn.prepare(&format!(
                "SELECT {} FROM {} WHERE {filter}",
                col.column, col.table
            ))?;
            let sample = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(ColumnRows {
                column: *col,
                total: total as usize,
                sample,
            });
        }
        Ok(out)
    })
    .await
}

/// Whether encrypted data exists in any registered column.
pub async fn has_ciphertext(db: &Database) -> Result<bool> {
    Ok(!collect_encrypted_rows(db).await?.is_empty())
}

/// Decide whether `config.toml` must keep a copy of the key in use. It is
/// dropped only when two independent copies remain and the key decrypts every
/// column. A different legacy value in the file decrypts nothing here (two
/// decrypting keys stop the boot earlier), so it is set aside in a side file
/// and the key in use gets the file copy it needs. Returns whether the file
/// keeps the key in use.
fn settle_disk_copy(
    dir: &Path,
    store: &KeyStore,
    key: &str,
    legacy_raw: Option<&str>,
    columns: &[ColumnRows],
) -> bool {
    if let Some(old) = legacy_raw.filter(|old| !crate::core::keyvault::same_key(old, key)) {
        match set_aside_retired_key(dir, old) {
            Ok(name) => tracing::warn!(
                "keystore: config.toml held another value than the key in use; it decrypts \
                 nothing and is kept aside as {name}"
            ),
            Err(e) => {
                // Never lose it: keep it in the file rather than the key in use.
                config::retain_disk_key(dir, old);
                tracing::error!("keystore: could not set aside the other config.toml key: {e}");
                return false;
            }
        }
    }
    // The file copy goes only when at least two independent copies remain:
    // a verified recovery passphrase and one vault, or two distinct tiers.
    let recovery = recovery::matches_key(dir, key);
    let copies = store.copies_of(key);
    if recovery != recovery::RecoveryMatch::Matches {
        tracing::warn!(
            "keystore: no recovery passphrase for this key ({recovery:?}) — every local copy is \
             kept; set one in Settings → Recovery so the key survives this machine"
        );
    }
    if enough_copies(dir, store, key) && decrypts_every_column(key, columns) {
        if config::release_disk_key(dir) {
            tracing::info!(
                "keystore: {copies} vault copies (recovery passphrase: {recovery:?}) and the key \
                 decrypts every encrypted column — config.toml no longer keeps a copy"
            );
        }
        false
    } else {
        config::retain_disk_key(dir, key);
        tracing::warn!(
            "keystore: fewer than two independent copies of the key (vaults, verified recovery \
             passphrase), or a column holds rows it cannot decrypt — config.toml keeps a copy so \
             the key is not lost"
        );
        true
    }
}

/// File-name prefix of a config.toml value that was not the key in use.
pub const RETIRED_KEY_PREFIX: &str = "config.toml.retired-key.";

/// Keep `value` (another key, or garbage) in `config.toml.retired-key.<ts>`
/// (owner-only) so nothing that sat in config.toml is ever dropped.
fn set_aside_retired_key(dir: &Path, value: &str) -> std::io::Result<String> {
    #[cfg(test)]
    if FAIL_SET_ASIDE.with(|f| f.get()) {
        return Err(std::io::Error::other("set-aside failure (test)"));
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
    let mut name = format!("{RETIRED_KEY_PREFIX}{stamp}");
    let mut n = 1;
    while dir.join(&name).exists() {
        name = format!("{RETIRED_KEY_PREFIX}{stamp}-{n}");
        n += 1;
    }
    std::fs::create_dir_all(dir)?;
    let content = format!(
        "encryption_secret = {}\n",
        toml::Value::String(value.to_string())
    );
    let tmp = dir.join(format!(".{name}.tmp"));
    crate::core::keyvault::write_private_atomic(&tmp, &dir.join(&name), content.as_bytes())?;
    Ok(name)
}

#[cfg(test)]
thread_local! {
    static FAIL_SET_ASIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The key must survive a restart before anything is encrypted under it: a
/// vault reads it back, or config.toml holds it (written now and read back).
fn ensure_durable_copy(dir: &Path, store: &KeyStore, config: &AppConfig, key: &str) -> Result<()> {
    if store.copies_of(key) >= 1 {
        return Ok(());
    }
    // Another value kept for config.toml (a set-aside that failed) is never
    // overwritten by the key in use.
    if config::retained_disk_key(dir)
        .is_some_and(|kept| !crate::core::keyvault::same_key(&kept, key))
    {
        return Err(KeyBootError::NoDurableCopy.into());
    }
    config::retain_disk_key(dir, key);
    // config.toml already holding it on disk is a durable copy: no rewrite
    // (which could fail on a full disk) is needed.
    if config::read_disk_key(dir)
        .ok()
        .flatten()
        .is_some_and(|on_disk| crate::core::keyvault::same_key(&on_disk, key))
    {
        return Ok(());
    }
    config::write_key_copy_now(dir, config)
        .and_then(|()| config::read_disk_key(dir))
        .ok()
        .flatten()
        .filter(|on_disk| crate::core::keyvault::same_key(on_disk, key))
        .map(|_| ())
        .ok_or_else(|| KeyBootError::NoDurableCopy.into())
}

/// Whether config.toml may go without its copy of `key`: two persisted vault
/// copies, or one plus a `recovery.key` verified for this key.
fn enough_copies(dir: &Path, store: &KeyStore, key: &str) -> bool {
    let copies = store.copies_of(key);
    copies >= 2
        || (copies >= 1 && recovery::matches_key(dir, key) == recovery::RecoveryMatch::Matches)
}

/// Mirror the resolved key into every writable vault, warning loudly if NO
/// durable backup could be written (WSL/Docker with no keychain + unwritable
/// sidecar) — the key would then live only in config.toml.
fn persist(store: &KeyStore, secret: &str) {
    let results = store.mirror(secret);
    let mut any_ok = false;
    for (name, res) in &results {
        match res {
            Ok(()) => any_ok = true,
            Err(e) => tracing::warn!("keystore: mirror key to {name} failed: {e}"),
        }
    }
    if !results.is_empty() && !any_ok {
        tracing::error!(
            "keystore: NO durable key backup available (every vault unwritable). The key \
             survives only in config.toml — set KRONN_ENCRYPTION_KEK or fix the data dir."
        );
    }
}

/// Reconcile against a caller-supplied [`KeyStore`] (tests inject a mock ladder).
///
/// `config.encryption_secret` is the in-process key every consumer reads; it is
/// never serialized, and `dir` names the data directory whose `config.toml`
/// copy [`settle_disk_copy`] keeps or drops.
pub async fn reconcile_with(
    config: &mut AppConfig,
    db: &Database,
    store: &KeyStore,
    dir: &Path,
) -> Result<KeyOutcome> {
    // Candidate keys, highest priority first: env → keychain → sidecar (the
    // vault ladder), then the legacy config.toml field. An unreadable vault
    // stops here, before any decision.
    let snapshot = store
        .snapshot()
        .map_err(|failure| KeyBootError::VaultUnreadable {
            vault: failure.vault,
            hint: vault_hint(failure.vault, failure.corrupted),
            error: failure.error,
        })?;
    let mut candidates: Vec<(String, &'static str)> = Vec::new();
    for (name, val) in snapshot {
        if let Some(v) = val {
            if !v.is_empty() {
                candidates.push((v, name));
            }
        }
    }
    // The raw config.toml value: a candidate when it is a key; whatever it is,
    // set aside (never dropped) when it is not the key in use.
    let legacy_raw = config
        .encryption_secret
        .clone()
        .filter(|k| !k.trim().is_empty());
    if let Some(legacy) = legacy_raw.clone() {
        candidates.push((legacy, "legacy-config"));
    }
    // One spelling per key (case, whitespace), and a value that is not a 32-byte
    // hex key can decrypt nothing and must never be adopted.
    let mut invalid_sources: Vec<&'static str> = Vec::new();
    let candidates: Vec<(String, &'static str)> = candidates
        .into_iter()
        .filter_map(|(v, src)| match crypto::canonical_secret(&v) {
            Ok(key) => Some((key, src)),
            Err(_) => {
                tracing::error!("keystore: {src} holds a value that is not a valid key — ignored");
                invalid_sources.push(src);
                None
            }
        })
        .collect();
    let mut candidates = candidates;
    // De-dup by value (a key present in several tiers is one candidate).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(v, _)| seen.insert(v.clone()));

    // Keys kept in files (config.toml backups, set-aside values) are the
    // lowest-priority, read-only candidates of every decision: rows under such
    // a key are then seen (SeveralKeys, retirable), never silently orphaned.
    let mut file_of: Vec<(String, String)> = Vec::new();
    for (key, source, file) in file_keys(dir) {
        if !candidates.iter().any(|(k, _)| *k == key) {
            candidates.push((key.clone(), source));
            file_of.push((key, file));
        }
    }
    let file_name = |key: &str| {
        file_of
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, f)| f.clone())
    };
    let mut columns = collect_encrypted_rows(db).await?;
    let mut decision = decide(&candidates, &columns);
    // One live key store decrypts data and the other decrypting keys survive
    // only in files Kronn keeps: move their rows under the live key (checked,
    // one transaction per key), keep the files, and say so.
    let mut moved: Vec<String> = Vec::new();
    if let Decision::Conflict(found) = &decision {
        let live: Vec<_> = found
            .iter()
            .filter(|(_, src, ..)| !FILE_SOURCES.contains(src))
            .collect();
        if let [(to, to_src, ..)] = live.as_slice() {
            for (from, src, fp, _) in found.iter().filter(|(_, s, ..)| FILE_SOURCES.contains(s)) {
                let file = file_name(from).unwrap_or_else(|| (*src).to_string());
                let report = reencrypt_rows(db, from, to).await.with_context(|| {
                    format!("re-encrypt the rows under key {fp} kept in {file}")
                })?;
                if let Err(e) = crate::core::credential_store::reencrypt_backup(dir, from, to) {
                    tracing::warn!(
                        "keystore: the encrypted config.toml backup stays under key {fp} for \
                         now ({e:#}); the next start moves it"
                    );
                }
                tracing::warn!(
                    "keystore: {} row(s) under key {fp}, kept only in {file}, were re-encrypted \
                     under the key from {to_src}; {file} is kept",
                    report.rewritten
                );
                moved.push(format!(
                    "{} row(s) moved from the key kept in {file} to the key in use",
                    report.rewritten
                ));
            }
            columns = collect_encrypted_rows(db).await?;
            decision = decide(&candidates, &columns);
        }
    }
    if let Decision::Conflict(found) = &decision {
        if let Some((from, to)) = reencrypt_request(found) {
            // The operator named the key to retire: move its rows, then decide again.
            let report = reencrypt_rows(db, &from, &to).await?;
            // The encrypted config.toml backup follows the rows; a failure here
            // is retried at the next start (reencrypt_backup_to below).
            if let Err(e) = crate::core::credential_store::reencrypt_backup(dir, &from, &to) {
                tracing::warn!(
                    "keystore: the encrypted config.toml backup stays under the retired key for \
                     now ({e:#}); the next start moves it. Free disk space if this repeats."
                );
            }
            tracing::warn!(
                "keystore: rows re-encrypted on request (KRONN_REENCRYPT_FROM): {report:?}"
            );
            columns = collect_encrypted_rows(db).await?;
            decision = decide(&candidates, &columns);
        }
    }

    let (key, outcome) = match decision {
        Decision::Mint => {
            tracing::info!("keystore: fresh install — minted a new encryption key");
            (crypto::generate_secret(), KeyOutcome::Minted)
        }
        Decision::Adopt(key, source) => {
            tracing::info!("keystore: adopted existing key from {source} (no encrypted data yet)");
            (key, KeyOutcome::Resolved { source })
        }
        Decision::Accept(key, source) => {
            tracing::info!("keystore: key from {source} decrypts existing data — accepted");
            (key, KeyOutcome::Resolved { source })
        }
        Decision::Conflict(found) => {
            let details = found
                .iter()
                .map(|(key, src, fp, n)| match file_name(key) {
                    Some(file) => format!("{src} ({file}) holds key {fp} (decrypts {n} row(s))"),
                    None => format!("{src} holds key {fp} (decrypts {n} row(s))"),
                })
                .collect::<Vec<_>>()
                .join("; ");
            return Err(KeyBootError::SeveralKeys {
                details,
                service: crate::core::keyvault::KEYCHAIN_SERVICE,
                account: crate::core::keyvault::KEYCHAIN_ACCOUNT,
            }
            .into());
        }
        Decision::Lock(n) => {
            // I1 + I3: do NOT mint, do NOT touch ciphertext. Boot continues so the
            // key can be restored from the UI, but fail closed: no key in memory,
            // so nothing new is encrypted under a key no vault holds.
            let names: Vec<String> = columns
                .iter()
                .map(|c| format!("{}.{} ({})", c.column.table, c.column.column, c.total))
                .collect();
            tracing::error!(
                "keystore: {n} encrypted row(s) present [{}] but NO available key decrypts \
                 them (config.toml backups were tried too). Booting in a LOCKED state — restore \
                 the correct key (keychain / sidecar / KRONN_ENCRYPTION_KEK / recovery \
                 passphrase) or re-enter the secrets. Nothing was overwritten.",
                names.join(", ")
            );
            config.encryption_secret = None;
            record_report(dir, &candidates, None, &invalid_sources, 0, false);
            return Ok(KeyOutcome::Locked { encrypted_rows: n });
        }
    };
    config.encryption_secret = Some(key.clone());
    persist(store, &key);
    let file_keeps_key = settle_disk_copy(dir, store, &key, legacy_raw.as_deref(), &columns);
    if let Err(e) = ensure_durable_copy(dir, store, config, &key) {
        config.encryption_secret = None;
        return Err(e);
    }
    // The encrypted config.toml backup follows the key in use (a retirement
    // that stopped half-way, or an older key).
    if let Err(e) = crate::core::credential_store::reencrypt_backup_to(dir, &candidates, &key) {
        tracing::warn!(
            "keystore: the encrypted config.toml backup could not be moved under the key in use \
             ({e:#}); it stays readable with its own key. Free disk space, then restart."
        );
    }
    record_report(
        dir,
        &candidates,
        Some(&key),
        &invalid_sources,
        store.copies_of(&key),
        file_keeps_key
            || config::retained_disk_key(dir)
                .is_some_and(|k| crate::core::keyvault::same_key(&k, &key)),
    );
    record_moves(dir, moved);
    Ok(outcome)
}

/// Sources whose keys sit in files kept by Kronn, read-only candidates.
const FILE_SOURCES: &[&str] = &["config-backup", "retired-key", "corrupt-config"];

/// `encryption_secret` values found in config.toml.backup, its rotated copies,
/// the set-aside `config.toml.retired-key.<ts>` and `config.toml.corrupt.<ts>`
/// files, canonical and de-duplicated: (key, source, file name).
fn file_keys(dir: &Path) -> Vec<(String, &'static str, String)> {
    let mut keys: Vec<(String, &'static str, String)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return keys;
    };
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let source = if name == "config.toml.backup" || name.starts_with("config.toml.backup.") {
            "config-backup"
        } else if name.starts_with(RETIRED_KEY_PREFIX) {
            "retired-key"
        } else if name.starts_with("config.toml.corrupt.") {
            "corrupt-config"
        } else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let value = text
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| {
                t.get("encryption_secret")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            })
            .or_else(|| config::salvage_key_line(&text));
        if let Some(key) = value.and_then(|v| crypto::canonical_secret(&v).ok()) {
            if !keys.iter().any(|(k, ..)| *k == key) {
                keys.push((key, source, name));
            }
        }
    }
    keys
}

/// The canonical keys the stores hold now, and the stores holding a non-key.
fn store_sources(store: &KeyStore) -> (Vec<(String, &'static str)>, Vec<&'static str>) {
    let mut keys = Vec::new();
    let mut invalid = Vec::new();
    for (name, value) in store.snapshot().unwrap_or_default() {
        let Some(v) = value.filter(|v| !v.trim().is_empty()) else {
            continue;
        };
        match crypto::canonical_secret(&v) {
            Ok(k) if !keys.iter().any(|(x, _): &(String, &str)| *x == k) => keys.push((k, name)),
            Ok(_) => {}
            Err(_) => invalid.push(name),
        }
    }
    (keys, invalid)
}

/// What the last boot found in the key stores of a data directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyReport {
    /// Persisted vault copies of the key in use.
    pub copies: usize,
    /// Sources holding a valid key other than the one in use. 0.14.2 would
    /// adopt such a key after a downgrade (see the key-management doc).
    pub stale_sources: Vec<&'static str>,
    /// Sources holding a value that is not a key.
    pub invalid_sources: Vec<&'static str>,
    /// config.toml keeps a copy of the key in use (the retention decision).
    pub file_keeps_key: bool,
    /// Rows this start moved from a key kept only in a file to the key in use.
    pub rows_moved_from_files: Vec<String>,
}

static REPORTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, KeyReport>>,
> = std::sync::LazyLock::new(Default::default);

fn record_report(
    dir: &Path,
    candidates: &[(String, &'static str)],
    key: Option<&str>,
    invalid_sources: &[&'static str],
    copies: usize,
    file_keeps_key: bool,
) {
    // Files Kronn keeps are not key stores an older version would adopt.
    let stale_sources: Vec<&'static str> = candidates
        .iter()
        .filter(|(cand, src)| key != Some(cand.as_str()) && !FILE_SOURCES.contains(src))
        .map(|(_, src)| *src)
        .collect();
    if !stale_sources.is_empty() {
        tracing::warn!(
            "keystore: {stale_sources:?} hold another key than the one in use; it is never \
             overwritten. Set a recovery passphrase before any downgrade (0.14.2 would adopt it)"
        );
    }
    REPORTS.lock().unwrap_or_else(|p| p.into_inner()).insert(
        dir.to_path_buf(),
        KeyReport {
            copies,
            stale_sources,
            invalid_sources: invalid_sources.to_vec(),
            file_keeps_key,
            rows_moved_from_files: Vec::new(),
        },
    );
}

fn record_moves(dir: &Path, moved: Vec<String>) {
    if let Some(r) = REPORTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_mut(dir)
    {
        r.rows_moved_from_files = moved;
    }
}

#[cfg(test)]
pub(crate) fn record_report_for_tests(
    dir: &Path,
    copies: usize,
    stale: &[&'static str],
    file_keeps_key: bool,
) {
    REPORTS.lock().unwrap_or_else(|p| p.into_inner()).insert(
        dir.to_path_buf(),
        KeyReport {
            copies,
            stale_sources: stale.to_vec(),
            invalid_sources: Vec::new(),
            file_keeps_key,
            rows_moved_from_files: Vec::new(),
        },
    );
}

/// The report of the last boot for `dir` (empty when none ran).
pub fn boot_report(dir: &Path) -> KeyReport {
    REPORTS
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(dir)
        .cloned()
        .unwrap_or_default()
}

/// Production entry point: reconcile against the standard vault ladder for the
/// current data dir. Call once at boot, right after `Database::open`; an `Err`
/// must stop the boot (its message says what to do).
pub async fn reconcile(config: &mut AppConfig, db: &Database) -> Result<KeyOutcome> {
    let dir = config::config_dir()?;
    let store = KeyStore::standard(&dir);
    reconcile_with(config, db, &store, &dir).await
}

// ── Recovery passphrase (P2) ────────────────────────────────────────────────

/// Minimum recovery-passphrase length. Since the wrapped blob travels in
/// exports, the passphrase is the only barrier against OFFLINE brute-force on a
/// leaked backup. Per modern guidance (NIST 800-63B): length over composition
/// rules — 12+ chars (ideally a few words) with Argon2id puts exhaustive search
/// out of practical reach, while staying memorable. No character-class rules:
/// they yield predictable "Password1!" patterns, not entropy.
pub const MIN_RECOVERY_PASSPHRASE_LEN: usize = 12;

/// Set (or replace) the recovery passphrase: wrap the CURRENT active encryption
/// key under `passphrase` (Argon2id) and persist the blob to the data dir. The
/// returned recovery code is the off-machine copy the user should save — with it
/// + the passphrase, the key survives total loss of the machine/keychain/dir.
///
/// Replacing an existing `recovery.key` requires `current_passphrase` to unwrap
/// it: otherwise any caller could swap the user's recovery for its own.
pub fn set_recovery_passphrase(
    config: &AppConfig,
    passphrase: &str,
    current_passphrase: Option<&str>,
    replace_unverified: bool,
) -> Result<String> {
    let dir = config::config_dir()?;
    set_recovery_passphrase_with(
        &dir,
        config,
        passphrase,
        current_passphrase,
        replace_unverified,
    )
}

#[cfg(test)]
pub(crate) fn set_recovery_passphrase_in(
    dir: &Path,
    config: &AppConfig,
    passphrase: &str,
    current_passphrase: Option<&str>,
) -> Result<String> {
    set_recovery_passphrase_with(dir, config, passphrase, current_passphrase, false)
}

/// `replace_unverified`: the user confirmed replacing a recovery.key that
/// predates 0.14.3 without its passphrase (it is kept, a restore still tries it).
pub(crate) fn set_recovery_passphrase_with(
    dir: &Path,
    config: &AppConfig,
    passphrase: &str,
    current_passphrase: Option<&str>,
    replace_unverified: bool,
) -> Result<String> {
    // Everything that can refuse comes first, so a refused call writes nothing.
    if passphrase.chars().count() < MIN_RECOVERY_PASSPHRASE_LEN {
        anyhow::bail!(
            "Recovery passphrase must be at least {MIN_RECOVERY_PASSPHRASE_LEN} characters \
             (a few words work well)"
        );
    }
    let key_hex = config.encryption_secret.clone().ok_or_else(|| {
        anyhow::anyhow!("no active encryption key to protect (token subsystem is locked?)")
    })?;
    let current = current_passphrase.unwrap_or_default();
    let state = recovery::matches_key(dir, &key_hex);
    // Replaced without the passphrase, the old blob kept: one for ANOTHER key
    // (cannot protect this one), one with a damaged payload (opens nothing),
    // or one predating 0.14.3 when the user confirmed it.
    let replace_kept = current.is_empty()
        && recovery::load_blob(dir).is_some()
        && match state {
            recovery::RecoveryMatch::OtherKey | recovery::RecoveryMatch::Unreadable => true,
            recovery::RecoveryMatch::Unverified => replace_unverified,
            _ => false,
        };
    if replace_kept {
        if let Some(existing) = recovery::load_blob(dir) {
            let kept = recovery::save_previous_blob(dir, &existing)
                .context("keep the replaced recovery.key")?;
            tracing::warn!("keystore: recovery.key ({state:?}) is kept as {kept} and replaced");
        }
    } else if recovery::is_configured(dir) {
        let existing = recovery::load_blob(dir).ok_or_else(|| {
            anyhow::anyhow!(
                "the existing recovery.key cannot be read, so it cannot be replaced from here — \
                 move it out of the Kronn data directory first if you no longer need it"
            )
        })?;
        if current.is_empty() {
            if state == recovery::RecoveryMatch::Unverified {
                anyhow::bail!(
                    "A recovery passphrase from before 0.14.3 is set: enter it, or confirm \
                     replacing it without it (the old file is kept and a restore still tries it)"
                );
            }
            anyhow::bail!(
                "A recovery passphrase is already set: enter the current one to replace it"
            );
        }
        let Ok(old_key) = recovery::unwrap_key(&existing, current) else {
            anyhow::bail!("The current recovery passphrase is incorrect — nothing was changed");
        };
        // A blob for another key may be the last copy of that key here: keep
        // it (re-encryption reads it) instead of replacing it for good.
        if !crate::core::keyvault::same_key(&old_key, &key_hex) {
            let kept = recovery::save_previous_blob(dir, &existing)
                .context("keep the replaced recovery.key, which wraps another key")?;
            tracing::warn!(
                "keystore: the replaced recovery.key wrapped another key; it is kept as {kept}"
            );
        }
    }
    let blob = recovery::wrap_key(&key_hex, passphrase).map_err(|e| anyhow::anyhow!(e))?;
    recovery::save_blob(dir, &blob).context("persist recovery sidecar")?;
    Ok(recovery::to_code(&blob))
}

/// Recover the encryption key from a passphrase when the token subsystem is
/// locked. The blob comes from a user-pasted `recovery_code` (survives data-dir
/// loss) or, failing that, the local `recovery.key` sidecar. The unwrapped key
/// is accepted ONLY if it actually decrypts existing ciphertext (or there's no
/// ciphertext yet), then set live and mirrored back into the vault ladder.
pub async fn recover_with_passphrase(
    config: &mut AppConfig,
    db: &Database,
    store: &KeyStore,
    passphrase: &str,
    recovery_code: Option<&str>,
    dir: &std::path::Path,
) -> Result<KeyOutcome> {
    let blobs = match recovery_code {
        Some(code) if !code.trim().is_empty() => {
            vec![recovery::from_code(code).map_err(|e| anyhow::anyhow!(e))?]
        }
        // The local recovery.key, then the replaced ones kept beside it (a
        // passphrase replaced without being entered still opens its file).
        _ => {
            let mut all: Vec<_> = recovery::load_blob(dir).into_iter().collect();
            all.extend(recovery::previous_blobs(dir));
            if all.is_empty() {
                anyhow::bail!("no recovery data — provide the recovery code you saved");
            }
            all
        }
    };
    let mut last_error = None;
    let mut opened = Vec::new();
    for blob in &blobs {
        match recovery::unwrap_key(blob, passphrase) {
            Ok(k) => opened.push((blob, k)),
            Err(e) => last_error = Some(e),
        }
    }
    if opened.is_empty() {
        return Err(anyhow::anyhow!(last_error.unwrap_or_default()));
    }
    // Several kept blobs may open with this passphrase: prefer the key in use,
    // then one that decrypts this instance's data.
    let columns = collect_encrypted_rows(db).await?;
    let pick = opened
        .iter()
        .position(|(_, k)| {
            config
                .encryption_secret
                .as_deref()
                .is_some_and(|active| crate::core::keyvault::same_key(active, k))
        })
        .or_else(|| {
            opened.iter().position(|(_, k)| {
                columns
                    .iter()
                    .any(|col| col.sample.iter().any(|enc| decrypts(enc, k)))
            })
        })
        .unwrap_or(0);
    let (blob, key) = opened.swap_remove(pick);
    upgrade_local_blob_fingerprint(dir, blob, &key);

    // Restoring is for a locked instance. On a running one, another key would
    // split the data between two keys; the same key changes nothing.
    if let Some(active) = config.encryption_secret.as_deref() {
        if crate::core::keyvault::same_key(active, &key) {
            return Ok(KeyOutcome::Resolved {
                source: "recovery-passphrase",
            });
        }
        anyhow::bail!(
            "Kronn is not locked: its encryption key is in use and this recovery code wraps \
             another key. Restoring it would split your secrets between two keys. To read secrets \
             imported from another machine, use 'Re-encrypt imported secrets' with that \
             machine's passphrase instead. Nothing was changed."
        );
    }

    // The recovered key must match THIS instance's data (unless there's none).
    if !columns.is_empty()
        && !columns
            .iter()
            .any(|col| col.sample.iter().any(|enc| decrypts(enc, &key)))
    {
        return Err(anyhow::anyhow!(
            "the recovered key does not decrypt this instance's data — wrong recovery code for this Kronn"
        ));
    }

    // Another value config.toml held (kept by the locked boot) is set aside
    // before the key replaces it; if that fails, nothing changes.
    let previous = config::retained_disk_key(dir);
    if let Some(old) = previous
        .as_deref()
        .filter(|old| !crate::core::keyvault::same_key(old, &key))
    {
        let name = set_aside_retired_key(dir, old).context(
            "config.toml holds another value that could not be set aside; nothing was changed",
        )?;
        tracing::warn!("keystore: config.toml held another value; it is kept aside as {name}");
        config::release_disk_key(dir);
    }
    config.encryption_secret = Some(key.clone());
    persist(store, &key);
    let file_keeps_key = !enough_copies(dir, store, &key);
    if file_keeps_key {
        // Same rule as the boot: without two copies, config.toml keeps one.
        config::retain_disk_key(dir, &key);
    }
    if let Err(e) = ensure_durable_copy(dir, store, config, &key) {
        config.encryption_secret = None;
        // A failed restore leaves the file copy as it found it (set aside
        // values are already safe in their own file).
        match previous.filter(|old| crate::core::keyvault::same_key(old, &key)) {
            Some(old) => config::retain_disk_key(dir, &old),
            None => {
                config::release_disk_key(dir);
            }
        }
        return Err(e);
    }
    // The status now describes the restored key, with what the key stores
    // still hold (a stale value is never overwritten, so it still warns).
    let (sources, invalid) = store_sources(store);
    record_report(
        dir,
        &sources,
        Some(&key),
        &invalid,
        store.copies_of(&key),
        file_keeps_key,
    );
    tracing::info!("keystore: encryption key restored from recovery passphrase");
    Ok(KeyOutcome::Resolved {
        source: "recovery-passphrase",
    })
}

/// A pre-0.14.3 `recovery.key` (no fingerprint) that the passphrase just
/// unwrapped gets its fingerprint, so it can count as a verified copy.
fn upgrade_local_blob_fingerprint(dir: &Path, used: &recovery::RecoveryBlob, key_hex: &str) {
    let Some(local) = recovery::load_blob(dir) else {
        return;
    };
    if local.fingerprint.is_some() || local.salt != used.salt || local.wrapped != used.wrapped {
        return;
    }
    if let Ok(fp) = crypto::key_fingerprint_hex(key_hex) {
        let upgraded = recovery::RecoveryBlob {
            fingerprint: Some(fp),
            ..local
        };
        if let Err(e) = recovery::save_blob(dir, &upgraded) {
            tracing::warn!("keystore: could not record the recovery.key fingerprint: {e}");
        }
    }
}

/// What [`reencrypt_rows`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reencrypted {
    /// Rows moved from the old key to the new one.
    pub rewritten: usize,
    /// Rows already under the new key, left as they are.
    pub already_current: usize,
    /// Rows neither key decrypts, left as they are.
    pub untouched: usize,
}

/// Re-encrypt every registered-column row that `from_hex` decrypts under
/// `to_hex`, in one transaction. Each rewritten row is read back and must
/// decrypt under `to_hex` to the same plaintext before the commit; rows the new
/// key already decrypts, or neither key does, are never written.
pub async fn reencrypt_rows(db: &Database, from_hex: &str, to_hex: &str) -> Result<Reencrypted> {
    let from = crypto::parse_secret(from_hex).map_err(anyhow::Error::msg)?;
    let to = crypto::parse_secret(to_hex).map_err(anyhow::Error::msg)?;
    if from == to {
        anyhow::bail!("the source and target keys are the same");
    }
    db.with_conn(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut report = Reencrypted::default();
        for col in ENCRYPTED_COLUMNS {
            let rows: Vec<(i64, String)> = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT rowid, {c} FROM {t} WHERE {c} IS NOT NULL AND {c} != ''",
                    c = col.column,
                    t = col.table
                ))?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (rowid, enc) in rows {
                if crypto::decrypt(&enc, &to).is_ok() {
                    report.already_current += 1;
                    continue;
                }
                let Ok(plain) = crypto::decrypt(&enc, &from) else {
                    report.untouched += 1;
                    continue;
                };
                let fresh = crypto::encrypt(&plain, &to).map_err(anyhow::Error::msg)?;
                tx.execute(
                    &format!("UPDATE {} SET {} = ?1 WHERE rowid = ?2", col.table, col.column),
                    rusqlite::params![fresh, rowid],
                )?;
                let back: String = tx.query_row(
                    &format!("SELECT {} FROM {} WHERE rowid = ?1", col.column, col.table),
                    [rowid],
                    |r| r.get(0),
                )?;
                if crypto::decrypt(&back, &to).ok().as_deref() != Some(plain.as_str()) {
                    anyhow::bail!(
                        "{}.{} row {rowid} did not read back under the new key; nothing was changed",
                        col.table,
                        col.column
                    );
                }
                report.rewritten += 1;
            }
        }
        tx.commit()?;
        Ok(report)
    })
    .await
}

/// Re-encrypt secrets imported from another machine under this instance's key,
/// using that machine's recovery passphrase. The instance key never changes.
/// The blob comes from `recovery_code` or, failing that, the newest imported one.
pub async fn reencrypt_imported(
    config: &AppConfig,
    db: &Database,
    passphrase: &str,
    recovery_code: Option<&str>,
    dir: &Path,
) -> Result<Reencrypted> {
    let active = config.encryption_secret.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "Kronn is locked: restore its own key first (Settings → Recovery), then re-encrypt \
             the imported secrets"
        )
    })?;
    // A pasted code, else every blob kept here: imported ones, replaced ones,
    // and the local recovery.key (rows may sit under an older local key).
    let candidates: Vec<recovery::RecoveryBlob> = match recovery_code {
        Some(code) if !code.trim().is_empty() => {
            vec![recovery::from_code(code).map_err(|e| anyhow::anyhow!(e))?]
        }
        _ => recovery::imported_blobs(dir)
            .into_iter()
            .chain(recovery::previous_blobs(dir))
            .chain(recovery::load_blob(dir))
            .collect(),
    };
    if candidates.is_empty() {
        anyhow::bail!("no imported recovery data — paste the source machine's recovery code");
    }
    let mut sources: Vec<String> = Vec::new();
    let mut unwrapped_any = false;
    for blob in &candidates {
        if let Ok(key) = recovery::unwrap_key(blob, passphrase) {
            unwrapped_any = true;
            if !crate::core::keyvault::same_key(&key, active) && !sources.contains(&key) {
                sources.push(key);
            }
        }
    }
    if !unwrapped_any {
        anyhow::bail!("Wrong recovery passphrase for the imported data");
    }
    let mut total = Reencrypted::default();
    for source in sources {
        total.rewritten += reencrypt_rows(db, &source, active).await?.rewritten;
    }
    // Counted once over every row after all passes, so nothing is counted twice.
    let columns = collect_encrypted_rows(db).await?;
    let rows: usize = columns.iter().map(|c| c.total).sum();
    let current: usize = columns
        .iter()
        .map(|c| c.sample.iter().filter(|enc| decrypts(enc, active)).count())
        .sum();
    total.already_current = current.saturating_sub(total.rewritten);
    total.untouched = rows - current;
    tracing::info!("keystore: imported secrets re-encrypted under this instance's key: {total:?}");
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keyvault::KeyVault;
    use crate::db::mcps;
    use std::collections::HashMap;

    /// A vault that always holds one fixed value — enough to drive `reconcile_with`.
    struct FixedVault {
        name: &'static str,
        val: String,
    }
    impl KeyVault for FixedVault {
        fn name(&self) -> &'static str {
            self.name
        }
        fn retrieve(&self) -> Result<Option<String>> {
            Ok(Some(self.val.clone()))
        }
        fn store(&self, _secret: &str) -> Result<()> {
            Ok(())
        }
    }

    fn encrypt_row(secret: &str, k: &str, v: &str) -> String {
        let mut env = HashMap::new();
        env.insert(k.to_string(), v.to_string());
        mcps::encrypt_env(&env, secret).unwrap()
    }

    /// Rows of the MCP column only, the shape `decide` used to receive.
    fn mcp_rows(rows: &[String]) -> Vec<ColumnRows> {
        if rows.is_empty() {
            return Vec::new();
        }
        vec![ColumnRows {
            column: ENCRYPTED_COLUMNS[0],
            total: rows.len(),
            sample: rows.to_vec(),
        }]
    }

    // ── decide(): the pure incident-proof core ──────────────────────────────

    #[test]
    fn empty_db_no_candidate_mints() {
        assert!(matches!(decide(&[], &mcp_rows(&[])), Decision::Mint));
    }

    #[test]
    fn empty_db_adopts_first_candidate() {
        match decide(&[("KKKK".into(), "keychain")], &mcp_rows(&[])) {
            Decision::Adopt(k, src) => {
                assert_eq!(k, "KKKK");
                assert_eq!(src, "keychain");
            }
            other => panic!("expected Adopt, got {other:?}"),
        }
    }

    /// THE incident regression: tokens were encrypted under key K, config.toml
    /// lost the key, but the vault (sidecar/keychain) still holds K → must
    /// resolve K by decrypt self-test, NOT mint over the data.
    #[test]
    fn rows_exist_and_vault_key_decrypts_is_accepted() {
        let k = crypto::generate_secret();
        let rows = vec![encrypt_row(&k, "TOKEN", "s3cr3t")];
        match decide(&[(k.clone(), "sidecar")], &mcp_rows(&rows)) {
            Decision::Accept(key, src) => {
                assert_eq!(key, k);
                assert_eq!(src, "sidecar");
            }
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    #[test]
    fn accept_skips_wrong_candidate_and_picks_the_decrypting_one() {
        let k = crypto::generate_secret();
        let wrong = crypto::generate_secret();
        let rows = vec![encrypt_row(&k, "A", "b")];
        // wrong is higher priority but doesn't decrypt → must be skipped.
        let cands = vec![(wrong, "env"), (k.clone(), "sidecar")];
        match decide(&cands, &mcp_rows(&rows)) {
            Decision::Accept(key, src) => {
                assert_eq!(key, k);
                assert_eq!(src, "sidecar");
            }
            other => panic!("expected Accept(sidecar), got {other:?}"),
        }
    }

    /// I1: rows exist, only a WRONG key (and nothing else) is available → LOCK,
    /// never mint, and `decide` never mutates the ciphertext.
    #[test]
    fn rows_exist_and_nothing_decrypts_locks_without_touching_data() {
        let k = crypto::generate_secret();
        let wrong = crypto::generate_secret();
        let enc = encrypt_row(&k, "TOKEN", "s3cr3t");
        let rows = vec![enc.clone()];
        match decide(&[(wrong, "legacy-config")], &mcp_rows(&rows)) {
            Decision::Lock(n) => assert_eq!(n, 1),
            other => panic!("expected Lock, got {other:?}"),
        }
        assert_eq!(rows[0], enc, "decide must never mutate ciphertext");
    }

    #[test]
    fn rows_exist_and_no_candidate_at_all_locks() {
        let k = crypto::generate_secret();
        let rows = vec![encrypt_row(&k, "T", "v")];
        assert!(matches!(decide(&[], &mcp_rows(&rows)), Decision::Lock(1)));
    }

    // ── reconcile_with(): end-to-end wiring against a real in-memory DB ──────

    #[tokio::test]
    async fn reconcile_resolves_orphaned_rows_from_vault_and_never_rewrites_ciphertext() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let enc = encrypt_row(&k, "TOKEN", "s3cr3t");
        let enc_for_insert = enc.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO mcp_servers (id, name, transport) VALUES ('s1','t','stdio')",
                [],
            )?;
            conn.execute(
                "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json) \
                 VALUES ('c1','s1','t', ?1, '[\"TOKEN\"]')",
                [enc_for_insert],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // config.toml lost the key; the vault (mock "sidecar") still holds K.
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![Box::new(FixedVault {
            name: "sidecar",
            val: k.clone(),
        })]);

        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "sidecar" });
        assert_eq!(
            cfg.encryption_secret.as_deref(),
            Some(k.as_str()),
            "reconcile must set the resolved key in the live config"
        );

        // The ciphertext row must be byte-identical — reconcile never rewrites it.
        let after = db
            .with_conn(|conn| Ok(mcps::list_configs(conn)?[0].env_encrypted.clone()))
            .await
            .unwrap();
        assert_eq!(
            after, enc,
            "reconcile must not rewrite ciphertext (orphans == 0)"
        );
    }

    /// A stored GitHub token is encrypted data too: with only that row and a
    /// key that does not decrypt it, reconcile locks instead of minting.
    #[tokio::test]
    async fn a_stored_github_token_alone_prevents_minting_over_it() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let cipher =
            crate::core::github_connection::encrypt_stored_token("github_pat_x", &k).unwrap();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at, updated_at) \
                 VALUES ('p1', 'p1', '/nowhere', datetime('now'), datetime('now'))",
                [],
            )?;
            crate::db::github_connections::set_mode(
                conn,
                "p1",
                crate::models::GithubConnectionMode::StoredToken,
                Some(&cipher),
            )
        })
        .await
        .unwrap();

        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let wrong = KeyStore::from_vaults(vec![Box::new(FixedVault {
            name: "sidecar",
            val: crypto::generate_secret(),
        })]);
        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &wrong, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Locked { encrypted_rows: 1 });

        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let right = KeyStore::from_vaults(vec![Box::new(FixedVault {
            name: "sidecar",
            val: k.clone(),
        })]);
        let outcome = reconcile_with(&mut cfg, &db, &right, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "sidecar" });
    }

    #[tokio::test]
    async fn reconcile_adopts_vault_key_on_empty_db_and_mirrors_it() {
        // Empty DB (no rows to validate against) + a key in the vault → Adopt it
        // (not mint a new one), set it live, and mirror it back.
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![Box::new(FixedVault {
            name: "keychain",
            val: k.clone(),
        })]);

        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "keychain" });
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));
    }

    /// I11: minting must still succeed and set the key even when NO vault can
    /// store it (WSL/Docker with no keychain + unwritable sidecar) — the "no
    /// durable backup" branch warns loudly but never fails the boot.
    #[tokio::test]
    async fn reconcile_mints_even_when_no_vault_can_store_the_backup() {
        struct FailStore;
        impl KeyVault for FailStore {
            fn name(&self) -> &'static str {
                "failstore"
            }
            fn retrieve(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn store(&self, _s: &str) -> Result<()> {
                anyhow::bail!("no writable backup medium")
            }
        }
        let db = Database::open_in_memory().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![Box::new(FailStore)]);

        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
        assert!(
            cfg.encryption_secret.is_some(),
            "mint sets the key even if backup fails"
        );
    }

    #[tokio::test]
    async fn reconcile_mints_on_a_truly_empty_db() {
        let db = Database::open_in_memory().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        // No vault holds a key, no rows exist → mint.
        let store = KeyStore::from_vaults(vec![]);
        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
        assert!(
            cfg.encryption_secret.is_some(),
            "a fresh key must be set on an empty install"
        );
    }

    #[tokio::test]
    async fn reconcile_locks_when_rows_exist_but_no_key_decrypts() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let enc = encrypt_row(&k, "TOKEN", "s3cr3t");
        let enc_for_insert = enc.clone();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO mcp_servers (id, name, transport) VALUES ('s1','t','stdio')",
                [],
            )?;
            conn.execute(
                "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json) \
                 VALUES ('c1','s1','t', ?1, '[\"TOKEN\"]')",
                [enc_for_insert],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        // Only a WRONG key is available anywhere.
        let wrong = crypto::generate_secret();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(wrong.clone());
        let store = KeyStore::from_vaults(vec![]);

        let tmp = tempfile::tempdir().unwrap();
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Locked { encrypted_rows: 1 });
        // Fail closed: no key in memory, ciphertext untouched.
        assert!(
            cfg.encryption_secret.is_none(),
            "a wrong key must not stay live"
        );
        let after = db
            .with_conn(|conn| Ok(mcps::list_configs(conn)?[0].env_encrypted.clone()))
            .await
            .unwrap();
        assert_eq!(after, enc, "locked state must not touch ciphertext");
    }

    // ── recovery passphrase integration ─────────────────────────────────────

    /// Seed a DB with one config whose env is encrypted under `secret`.
    async fn seed_row(db: &Database, secret: &str) {
        let enc = encrypt_row(secret, "TOKEN", "s3cr3t");
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO mcp_servers (id, name, transport) VALUES ('s1','t','stdio')",
                [],
            )?;
            conn.execute(
                "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json) \
                 VALUES ('c1','s1','t', ?1, '[\"TOKEN\"]')",
                [enc],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn set_recovery_passphrase_enforces_min_length() {
        // The blob travels in exports → the passphrase is the only barrier
        // against offline brute-force. Backend must enforce, not just the UI.
        let cfg = config::default_config();
        let dir = tempfile::tempdir().unwrap();
        let err = set_recovery_passphrase_in(dir.path(), &cfg, "short-pass", None).unwrap_err();
        assert!(err.to_string().contains("at least 12"), "unexpected: {err}");
        // Restore, by contrast, must accept ANY passphrase (whatever was set
        // before the policy existed) — enforced only at set-time.
    }

    #[tokio::test]
    async fn recover_via_code_restores_a_locked_key() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;

        // The user saved this recovery code earlier (wrap of K under a passphrase).
        let code = recovery::to_code(&recovery::wrap_key(&k, "my-pass").unwrap());

        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None; // locked
        let store = KeyStore::from_vaults(vec![]);

        let outcome =
            recover_with_passphrase(&mut cfg, &db, &store, "my-pass", Some(&code), dir.path())
                .await
                .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "recovery-passphrase"
            }
        );
        assert_eq!(
            cfg.encryption_secret.as_deref(),
            Some(k.as_str()),
            "key restored live"
        );
    }

    #[tokio::test]
    async fn recover_via_sidecar_when_no_code_pasted() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;

        let dir = tempfile::tempdir().unwrap();
        recovery::save_blob(dir.path(), &recovery::wrap_key(&k, "pw").unwrap()).unwrap();

        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![]);

        let outcome = recover_with_passphrase(&mut cfg, &db, &store, "pw", None, dir.path())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "recovery-passphrase"
            }
        );
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));
    }

    #[tokio::test]
    async fn recover_rejects_wrong_passphrase() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let code = recovery::to_code(&recovery::wrap_key(&k, "right").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![]);

        let res =
            recover_with_passphrase(&mut cfg, &db, &store, "wrong", Some(&code), dir.path()).await;
        assert!(res.is_err(), "wrong passphrase must not recover");
        assert!(
            cfg.encryption_secret.is_none(),
            "no key set on failed recovery"
        );
    }

    #[tokio::test]
    async fn recover_rejects_key_that_does_not_match_this_instance() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        // A recovery code for a DIFFERENT key (another Kronn) unwraps fine but
        // must be refused because it doesn't decrypt THIS instance's data.
        let other = crypto::generate_secret();
        let code = recovery::to_code(&recovery::wrap_key(&other, "pw").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![]);

        let res =
            recover_with_passphrase(&mut cfg, &db, &store, "pw", Some(&code), dir.path()).await;
        assert!(
            res.is_err(),
            "a key that can't decrypt this instance's data must be refused"
        );
    }

    // ── KT-1007: every encrypted column, vault errors, config.toml copy ──────

    /// A vault whose read always fails while holding a value, and that records
    /// any write: a denied keychain prompt.
    struct DeniedVault {
        writes: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }
    impl KeyVault for DeniedVault {
        fn name(&self) -> &'static str {
            "keychain"
        }
        fn retrieve(&self) -> Result<Option<String>> {
            Err(crate::core::keyvault::VaultError::Denied("user denied the prompt".into()).into())
        }
        fn store(&self, secret: &str) -> Result<()> {
            self.writes.lock().unwrap().push(secret.to_string());
            Ok(())
        }
    }

    /// Writable in-memory vault that can be inspected afterwards.
    struct MemVault {
        name: &'static str,
        val: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    }
    impl KeyVault for MemVault {
        fn name(&self) -> &'static str {
            self.name
        }
        fn retrieve(&self) -> Result<Option<String>> {
            Ok(self.val.lock().unwrap().clone())
        }
        fn store(&self, secret: &str) -> Result<()> {
            *self.val.lock().unwrap() = Some(secret.to_string());
            Ok(())
        }
    }

    fn mem_vault(
        name: &'static str,
        val: Option<&str>,
    ) -> (
        Box<dyn KeyVault>,
        std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ) {
        let cell = std::sync::Arc::new(std::sync::Mutex::new(val.map(str::to_string)));
        (
            Box::new(MemVault {
                name,
                val: cell.clone(),
            }),
            cell,
        )
    }

    async fn seed_snapshot(db: &Database, secret: &str) {
        let key = crypto::parse_secret(secret).unwrap();
        let enc = crypto::encrypt("{\"TOKEN\":\"v\"}", &key).unwrap();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO execution_variable_snapshots (id, run_kind, run_id, environment_ref, resolved_at, retention_days, values_encrypted, fingerprint, provenance_json) \
                 VALUES ('snap1', 'workflow', 'run1', 'default', '2026-10-01T00:00:00Z', 30, ?1, 'fp', '[]')",
                [enc],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    async fn seed_credential(db: &Database, secret: &str) {
        let key = crypto::parse_secret(secret).unwrap();
        let enc = crypto::encrypt("sk-ant-1", &key).unwrap();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO stored_credentials (kind, id, name, provider, active, position, value_encrypted) \
                 VALUES ('provider_key', 'k1', 'n', 'anthropic', 1, 0, ?1)",
                [enc],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn denied_keychain_at_boot_stops_without_minting_or_writing() {
        let db = Database::open_in_memory().unwrap();
        let writes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let store = KeyStore::from_vaults(vec![Box::new(DeniedVault {
            writes: writes.clone(),
        })]);
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;

        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("keychain"), "{msg}");
        assert!(
            msg.contains("KRONN_USE_KEYCHAIN=0"),
            "actionable hint missing: {msg}"
        );
        assert!(msg.contains("Nothing was changed"), "{msg}");
        assert!(cfg.encryption_secret.is_none(), "no key may be minted");
        assert!(
            writes.lock().unwrap().is_empty(),
            "the denied vault must not be written"
        );
    }

    #[tokio::test]
    async fn unreadable_sidecar_hint_names_the_file() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(crate::core::keyvault::SIDECAR_FILENAME)).unwrap();
        let store = KeyStore::from_vaults(vec![Box::new(
            crate::core::keyvault::SidecarFile::in_dir(tmp.path()),
        )]);
        let mut cfg = config::default_config();
        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("encryption_key file"), "{err}");
    }

    #[tokio::test]
    async fn snapshot_ciphertext_alone_refuses_to_mint() {
        // Zero MCP rows, one execution-variable snapshot under K, no key
        // anywhere: the old reconciler minted here and orphaned the snapshot.
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_snapshot(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![]);

        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Locked { encrypted_rows: 1 });
        assert!(cfg.encryption_secret.is_none());
        assert!(has_ciphertext(&db).await.unwrap());
    }

    #[tokio::test]
    async fn stored_credential_ciphertext_alone_refuses_to_mint_or_adopt() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_credential(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        // A different key in a vault: it must not be adopted over the data.
        let (vault, cell) = mem_vault("sidecar", Some(&crypto::generate_secret()));
        let before = cell.lock().unwrap().clone();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![vault]);

        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Locked { encrypted_rows: 1 });
        assert_eq!(
            *cell.lock().unwrap(),
            before,
            "locked boot must not rewrite a vault"
        );
    }

    #[tokio::test]
    async fn legacy_config_key_moves_to_the_vaults_and_leaves_config_toml_with_two_copies() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        seed_snapshot(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k); // as load() does for a 0.14.2 file
        let (keychain, _kc) = mem_vault("keychain", None);
        let (vault, cell) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![keychain, vault]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());

        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "legacy-config"
            }
        );
        assert_eq!(
            cell.lock().unwrap().as_deref(),
            Some(k.as_str()),
            "mirrored first"
        );
        assert_eq!(
            config::retained_disk_key(tmp.path()),
            None,
            "then dropped from config.toml"
        );
    }

    // ── restore vs re-encryption (imports carrying another machine's key) ──

    /// An unlocked instance holding rows under its key K and imported rows
    /// under K_src: restoring K_src is refused and changes nothing; the
    /// re-encryption path brings every row under K and the reboot resolves.
    #[tokio::test]
    async fn restore_on_an_unlocked_instance_is_refused_and_reencryption_converges() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let k_src = crypto::generate_secret();
        seed_row(&db, &k).await;
        seed_snapshot(&db, &k_src).await; // imported, under the source key
        let tmp = tempfile::tempdir().unwrap();
        let (sidecar, cell) = mem_vault("sidecar", Some(&k));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));
        let retained = config::retained_disk_key(tmp.path());

        let imported = recovery::wrap_key(&k_src, "source pass").unwrap();
        recovery::save_imported_blob(tmp.path(), &imported).unwrap();
        let code = recovery::to_code(&imported);
        let err = recover_with_passphrase(
            &mut cfg,
            &db,
            &store,
            "source pass",
            Some(&code),
            tmp.path(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("not locked"), "{err}");
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));
        assert_eq!(cell.lock().unwrap().as_deref(), Some(k.as_str()));
        assert_eq!(config::retained_disk_key(tmp.path()), retained);

        // Same key: accepted, nothing changes.
        let own = recovery::to_code(&recovery::wrap_key(&k, "own pass").unwrap());
        recover_with_passphrase(&mut cfg, &db, &store, "own pass", Some(&own), tmp.path())
            .await
            .unwrap();
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));

        // Re-encryption with the source passphrase (newest imported blob).
        let wrong = reencrypt_imported(&cfg, &db, "nope", None, tmp.path()).await;
        assert!(wrong.is_err());
        let report = reencrypt_imported(&cfg, &db, "source pass", None, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            report,
            Reencrypted {
                rewritten: 1,
                already_current: 1,
                untouched: 0
            }
        );
        let again = reencrypt_imported(&cfg, &db, "source pass", None, tmp.path())
            .await
            .unwrap();
        assert_eq!(again.rewritten, 0, "idempotent");
        // Every row now decrypts under K; the snapshot plaintext is intact.
        let columns = collect_encrypted_rows(&db).await.unwrap();
        assert!(decrypts_every_column(&k, &columns));
        let enc: String = db
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT values_encrypted FROM execution_variable_snapshots",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            crypto::decrypt(&enc, &crypto::parse_secret(&k).unwrap()).unwrap(),
            "{\"TOKEN\":\"v\"}"
        );
        // A reboot with both keys around resolves to K.
        let mut fresh = config::default_config();
        fresh.encryption_secret = Some(k_src.clone());
        let outcome = reconcile_with(&mut fresh, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "sidecar" });

        // A locked instance must restore its own key first.
        let mut locked = config::default_config();
        locked.encryption_secret = None;
        assert!(
            reencrypt_imported(&locked, &db, "source pass", None, tmp.path())
                .await
                .unwrap_err()
                .to_string()
                .contains("locked")
        );
    }

    #[tokio::test]
    async fn reencrypt_rows_refuses_identical_keys_and_reports_rows_left_alone() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        assert!(reencrypt_rows(&db, &k, &k).await.is_err());
        seed_row(&db, &crypto::generate_secret()).await; // a third key
        let r = reencrypt_rows(&db, &crypto::generate_secret(), &k)
            .await
            .unwrap();
        assert_eq!(
            r,
            Reencrypted {
                rewritten: 0,
                already_current: 0,
                untouched: 1
            }
        );
    }

    /// Two decrypting keys + KRONN_REENCRYPT_FROM naming one: its rows move
    /// under the other, the boot resolves, and no vault is written or emptied.
    #[tokio::test]
    #[serial_test::serial]
    async fn reencrypt_from_resolves_two_keys_without_deleting_any() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, kc) = mem_vault("keychain", Some(&k1));
        let (sidecar, sc) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;

        std::env::set_var(
            ENV_REENCRYPT_FROM,
            crypto::key_fingerprint_hex(&k2).unwrap(),
        );
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(ENV_REENCRYPT_FROM);
        assert_eq!(
            outcome.unwrap(),
            KeyOutcome::Resolved { source: "keychain" }
        );
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k1.as_str()));
        assert_eq!(kc.lock().unwrap().as_deref(), Some(k1.as_str()));
        assert_eq!(
            sc.lock().unwrap().as_deref(),
            Some(k2.as_str()),
            "kept, not deleted"
        );
        assert!(decrypts_every_column(
            &k1,
            &collect_encrypted_rows(&db).await.unwrap()
        ));
    }

    /// The same key spelled in upper case (env) and lower case (sidecar) is one
    /// key: two boots in a row resolve, never "several keys".
    #[tokio::test]
    #[serial_test::serial]
    async fn one_key_in_two_spellings_is_one_candidate() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let (sidecar, cell) = mem_vault("sidecar", Some(&format!(" {k}\n")));
        let store = KeyStore::from_vaults(vec![sidecar]);
        std::env::set_var(crate::core::keyvault::ENV_KEK, k.to_uppercase());
        let mut outcomes = Vec::new();
        for _ in 0..2 {
            let mut cfg = config::default_config();
            cfg.encryption_secret = Some(k.to_uppercase());
            outcomes.push(reconcile_with(&mut cfg, &db, &store, tmp.path()).await);
            assert_eq!(
                cfg.encryption_secret.as_deref(),
                Some(k.as_str()),
                "canonical"
            );
        }
        std::env::remove_var(crate::core::keyvault::ENV_KEK);
        for outcome in outcomes {
            assert!(matches!(outcome.unwrap(), KeyOutcome::Resolved { .. }));
        }
        assert_eq!(
            cell.lock().unwrap().as_deref(),
            Some(format!(" {k}\n").as_str()),
            "not rewritten"
        );
        assert_eq!(store.copies_of(&k.to_uppercase()), 1);
    }

    // ── review round 3 ──────────────────────────────────────────────────────

    /// A failure in the middle of `reencrypt_rows` rolls every row back.
    #[tokio::test]
    async fn reencrypt_rows_rolls_back_on_a_mid_transaction_failure() {
        let db = Database::open_in_memory().unwrap();
        let from = crypto::generate_secret();
        let to = crypto::generate_secret();
        seed_row(&db, &from).await; // mcp_configs: rewritten first
        seed_snapshot(&db, &from).await; // its update is made to fail
        let before: String = db
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT env_encrypted FROM mcp_configs WHERE id = 'c1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        db.with_conn(|c| {
            c.execute_batch(
                "CREATE TRIGGER fail_snapshot BEFORE UPDATE ON execution_variable_snapshots \
                 BEGIN SELECT RAISE(ABORT, 'disk full'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(reencrypt_rows(&db, &from, &to).await.is_err());
        let after: String = db
            .with_conn(|c| {
                Ok(c.query_row(
                    "SELECT env_encrypted FROM mcp_configs WHERE id = 'c1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(after, before, "the first rewrite was rolled back");
        assert!(decrypts_every_column(
            &from,
            &collect_encrypted_rows(&db).await.unwrap()
        ));
    }

    /// C2-02 — a restore while both vaults hold a stale key keeps the restored
    /// key in config.toml (0 vault copies), so the next start resolves.
    #[tokio::test]
    async fn a_restore_with_stale_vaults_keeps_the_key_in_config_toml() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let bad = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        recovery::save_blob(tmp.path(), &recovery::wrap_key(&k, "pass").unwrap()).unwrap();
        let (keychain, _kc) = mem_vault("keychain", Some(&bad));
        let (sidecar, _sc) = mem_vault("sidecar", Some(&bad));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        assert!(matches!(
            reconcile_with(&mut cfg, &db, &store, tmp.path())
                .await
                .unwrap(),
            KeyOutcome::Locked { .. }
        ));
        recover_with_passphrase(&mut cfg, &db, &store, "pass", None, tmp.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k.clone()));
        let mut next = config::default_config();
        next.encryption_secret = config::retained_disk_key(tmp.path());
        assert_eq!(
            reconcile_with(&mut next, &db, &store, tmp.path())
                .await
                .unwrap(),
            KeyOutcome::Resolved {
                source: "legacy-config"
            }
        );
    }

    /// C2-09 — replacing a recovery.key that wraps another key keeps that blob.
    #[test]
    fn replacing_a_recovery_key_for_another_key_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let k_old = crypto::generate_secret();
        recovery::save_blob(dir.path(), &recovery::wrap_key(&k_old, "old pass").unwrap()).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(crypto::generate_secret());
        set_recovery_passphrase_in(dir.path(), &cfg, "new passphrase!", Some("old pass")).unwrap();
        let kept = recovery::previous_blobs(dir.path());
        assert_eq!(kept.len(), 1);
        assert_eq!(recovery::unwrap_key(&kept[0], "old pass").unwrap(), k_old);
        // Same key: nothing kept aside.
        let dir2 = tempfile::tempdir().unwrap();
        let k = cfg.encryption_secret.clone().unwrap();
        recovery::save_blob(dir2.path(), &recovery::wrap_key(&k, "old pass").unwrap()).unwrap();
        set_recovery_passphrase_in(dir2.path(), &cfg, "new passphrase!", Some("old pass")).unwrap();
        assert!(recovery::previous_blobs(dir2.path()).is_empty());
    }

    /// C2-10 — rows under an older local key whose recovery.key wraps it are
    /// re-encrypted without pasting a code.
    #[tokio::test]
    async fn reencrypt_also_reads_the_local_recovery_key() {
        let db = Database::open_in_memory().unwrap();
        let k_old = crypto::generate_secret();
        let k = crypto::generate_secret();
        seed_row(&db, &k_old).await;
        let tmp = tempfile::tempdir().unwrap();
        recovery::save_blob(tmp.path(), &recovery::wrap_key(&k_old, "old pass").unwrap()).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        let r = reencrypt_imported(&cfg, &db, "old pass", None, tmp.path())
            .await
            .unwrap();
        assert_eq!(r.rewritten, 1);
        assert!(decrypts_every_column(
            &k,
            &collect_encrypted_rows(&db).await.unwrap()
        ));
    }

    /// C2-18 — one damaged byte in the payload: no longer a verified copy.
    #[test]
    fn a_damaged_recovery_payload_is_not_a_verified_copy() {
        let dir = tempfile::tempdir().unwrap();
        let k = crypto::generate_secret();
        let blob = recovery::wrap_key(&k, "pw").unwrap();
        recovery::save_blob(dir.path(), &blob).unwrap();
        assert_eq!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Matches
        );
        let code = recovery::to_code(&blob);
        let mut parts: Vec<String> = code.split('.').map(str::to_string).collect();
        let mut w: Vec<char> = parts[2].chars().collect();
        w[5] = if w[5] == 'A' { 'B' } else { 'A' };
        parts[2] = w.into_iter().collect();
        std::fs::write(
            dir.path().join(recovery::RECOVERY_FILENAME),
            parts.join("."),
        )
        .unwrap();
        assert_ne!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Matches
        );
        // A fingerprint without its checksum is unverified, never Matches.
        std::fs::write(
            dir.path().join(recovery::RECOVERY_FILENAME),
            parts[..4].join("."),
        )
        .unwrap();
        assert_ne!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Matches
        );
    }

    /// C2-19 — concurrent sets leave a recovery.key that parses and is one of
    /// the returned codes.
    #[test]
    fn concurrent_recovery_sets_never_leave_a_torn_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(crypto::generate_secret());
        let codes: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..6)
                .map(|i| {
                    let cfg = cfg.clone();
                    let dir = dir.path().to_path_buf();
                    scope.spawn(move || {
                        let blob = recovery::wrap_key(
                            cfg.encryption_secret.as_deref().unwrap(),
                            &format!("passphrase number {i}"),
                        )
                        .unwrap();
                        recovery::save_blob(&dir, &blob).unwrap();
                        recovery::to_code(&blob)
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let on_disk =
            std::fs::read_to_string(dir.path().join(recovery::RECOVERY_FILENAME)).unwrap();
        assert!(recovery::from_code(&on_disk).is_ok());
        assert!(
            codes.contains(&on_disk),
            "the file is one writer's whole blob"
        );
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    /// C2-23 — a non-text sidecar is reported as corrupted with what to do.
    #[tokio::test]
    async fn a_non_utf8_sidecar_is_reported_as_corrupted() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(crate::core::keyvault::SIDECAR_FILENAME),
            [0xff, 0xfe, 0x00, 0xc3],
        )
        .unwrap();
        let store = KeyStore::from_vaults(vec![Box::new(
            crate::core::keyvault::SidecarFile::in_dir(tmp.path()),
        )]);
        let mut cfg = config::default_config();
        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("corrupted") && err.contains("move it aside"),
            "{err}"
        );
    }

    /// C2-24 — three keys: retiring one per start converges.
    #[tokio::test]
    #[serial_test::serial]
    async fn three_keys_converge_one_retirement_per_start() {
        let db = Database::open_in_memory().unwrap();
        let (k1, k2, k3) = (
            crypto::generate_secret(),
            crypto::generate_secret(),
            crypto::generate_secret(),
        );
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        seed_credential(&db, &k3).await;
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&k1));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let boot = |retire: &str| {
            std::env::set_var(
                ENV_REENCRYPT_FROM,
                crypto::key_fingerprint_hex(retire).unwrap(),
            );
        };
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k3.clone());
        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("repeat once per key"), "{err}");
        boot(&k3);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k3.clone());
        let second = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        assert!(
            second.is_err(),
            "two keys still decrypt data after one retirement"
        );
        boot(&k2);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k3.clone());
        let third = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(ENV_REENCRYPT_FROM);
        assert_eq!(third.unwrap(), KeyOutcome::Resolved { source: "keychain" });
        assert!(decrypts_every_column(
            &k1,
            &collect_encrypted_rows(&db).await.unwrap()
        ));
    }

    /// C2-25 — a second key whose rows are all older than 10,000 newer rows
    /// is still detected.
    #[tokio::test]
    async fn a_second_key_behind_many_newer_rows_is_detected() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_credential(&db, &k2).await; // the oldest row
        let enc = crypto::encrypt("v", &crypto::parse_secret(&k1).unwrap()).unwrap();
        db.with_conn(move |conn| {
            let tx = conn.unchecked_transaction()?;
            for i in 0..10_001 {
                tx.execute(
                    "INSERT INTO stored_credentials (kind, id, value_encrypted) VALUES ('provider_key', ?1, ?2)",
                    rusqlite::params![format!("n{i}"), enc],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&k1));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Several encryption keys"), "{err}");
    }

    /// C2-26 — retiring a key moves the encrypted config backup with the rows.
    #[tokio::test]
    #[serial_test::serial]
    async fn retiring_a_key_moves_the_config_backup_too() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp
            .path()
            .join(crate::core::credential_store::BACKUP_FILENAME);
        std::fs::write(
            &backup,
            crypto::encrypt("old config", &crypto::parse_secret(&k2).unwrap()).unwrap(),
        )
        .unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&k1));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        std::env::set_var(
            ENV_REENCRYPT_FROM,
            crypto::key_fingerprint_hex(&k2).unwrap(),
        );
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(ENV_REENCRYPT_FROM);
        outcome.unwrap();
        assert_eq!(
            crate::core::credential_store::read_backup(&backup, &k1).unwrap(),
            "old config"
        );
    }

    /// C2-27 — the env variable is not a persisted copy: env K + sidecar K
    /// without a passphrase keeps config.toml's copy.
    #[tokio::test]
    #[serial_test::serial]
    async fn an_env_key_does_not_count_as_a_persisted_copy() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k);
        let (sidecar, _s) = mem_vault("sidecar", Some(&k));
        let store = KeyStore::from_vaults(vec![sidecar]);
        std::env::set_var(crate::core::keyvault::ENV_KEK, &k);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(crate::core::keyvault::ENV_KEK);
        outcome.unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k));
    }

    /// C2-14 / C2-21 — a vault holding another key is reported, and two vault
    /// copies count as two.
    #[tokio::test]
    async fn the_boot_report_names_stale_and_invalid_sources_and_counts_copies() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&crypto::generate_secret()));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some("not-a-key".into());
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        let report = boot_report(tmp.path());
        assert_eq!(report.stale_sources, vec!["keychain"]);
        assert_eq!(report.invalid_sources, vec!["legacy-config"]);
        assert_eq!(report.copies, 1);

        let tmp2 = tempfile::tempdir().unwrap();
        let (keychain, _c) = mem_vault("keychain", Some(&k));
        let (sidecar, _d) = mem_vault("sidecar", Some(&k));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp2.path())
            .await
            .unwrap();
        assert_eq!(boot_report(tmp2.path()).copies, 2);
    }

    /// A corrupted vault value with multi-byte characters never panics the boot.
    #[tokio::test]
    async fn a_multibyte_vault_value_is_ignored_without_panicking() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let mut corrupted = "a".repeat(61);
        corrupted.push_str("éb");
        let (sidecar, _cell) = mem_vault("sidecar", Some(&corrupted));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some("日本".repeat(16));
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
    }

    #[tokio::test]
    async fn an_invalid_vault_value_is_never_adopted() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let (sidecar, cell) = mem_vault("sidecar", Some("not-a-key"));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
        let key = cfg.encryption_secret.clone().unwrap();
        assert!(crypto::parse_secret(&key).is_ok());
        assert_eq!(
            cell.lock().unwrap().as_deref(),
            Some("not-a-key"),
            "not overwritten"
        );
    }

    #[tokio::test]
    async fn restoring_with_an_old_blob_records_its_fingerprint() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let old = recovery::RecoveryBlob {
            fingerprint: None,
            ..recovery::wrap_key(&k, "pw").unwrap()
        };
        recovery::save_blob(tmp.path(), &old).unwrap();
        assert_eq!(
            recovery::matches_key(tmp.path(), &k),
            recovery::RecoveryMatch::Unverified
        );
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        recover_with_passphrase(
            &mut cfg,
            &db,
            &KeyStore::from_vaults(vec![]),
            "pw",
            None,
            tmp.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            recovery::matches_key(tmp.path(), &k),
            recovery::RecoveryMatch::Matches
        );
    }

    /// Sidecar-only ladder (Linux, WSL, Docker, dev builds) and no recovery
    /// passphrase: the sidecar would be the single copy, so config.toml keeps it.
    #[tokio::test]
    async fn a_sidecar_only_ladder_without_a_passphrase_keeps_the_key_in_config_toml() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k);
        let (vault, cell) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![vault]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(cell.lock().unwrap().as_deref(), Some(k.as_str()));
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k.clone()));

        // A recovery.key for ANOTHER key is no recovery copy of this one.
        let other = recovery::wrap_key(&crypto::generate_secret(), "a passphrase").unwrap();
        recovery::save_blob(tmp.path(), &other).unwrap();
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k.clone()));

        // A verified passphrase for this key is: the file copy may go.
        recovery::save_blob(tmp.path(), &recovery::wrap_key(&k, "a passphrase").unwrap()).unwrap();
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), None);
    }

    /// A stale key in the keychain is not a copy: sidecar K alone → keep the file.
    #[tokio::test]
    async fn a_stale_keychain_key_does_not_count_as_a_copy() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k);
        let (keychain, kc) = mem_vault("keychain", Some(&crypto::generate_secret()));
        let (sidecar, _sc) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_ne!(
            kc.lock().unwrap().as_deref(),
            Some(k.as_str()),
            "never overwritten"
        );
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k));
    }

    /// Without a recovery passphrase, a keychain-only copy is not enough to
    /// drop the config.toml copy; once a passphrase exists it is.
    #[tokio::test]
    async fn a_keychain_only_copy_needs_a_verified_passphrase_to_drop_the_config_copy() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k);
        let (vault, _cell) = mem_vault("keychain", None);
        let store = KeyStore::from_vaults(vec![vault]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());

        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            config::retained_disk_key(tmp.path()),
            Some(k.clone()),
            "keychain alone + no recovery passphrase: keep the file copy"
        );

        recovery::save_blob(tmp.path(), &recovery::wrap_key(&k, "a passphrase").unwrap()).unwrap();
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), None);
    }

    /// Keychain holds K1, sidecar holds K2, each with rows only it decrypts:
    /// the boot stops and both keys stay where they were.
    #[tokio::test]
    async fn two_keys_each_decrypting_data_stop_the_boot_without_any_write() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, kc) = mem_vault("keychain", Some(&k1));
        let (sidecar, sc) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;

        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("keychain holds key") && msg.contains("sidecar holds key"),
            "{msg}"
        );
        assert!(msg.contains("decrypts 1 row(s)"), "{msg}");
        assert!(
            !msg.contains(&k1) && !msg.contains(&k2),
            "never print a key"
        );
        assert_eq!(kc.lock().unwrap().as_deref(), Some(k1.as_str()));
        assert_eq!(sc.lock().unwrap().as_deref(), Some(k2.as_str()));
        assert!(cfg.encryption_secret.is_none());
        assert_eq!(
            config::retained_disk_key(tmp.path()),
            None,
            "nothing retained or written"
        );
    }

    /// No ciphertext: the first key is adopted, but a vault holding another key
    /// is not overwritten.
    #[tokio::test]
    async fn adopting_a_key_never_overwrites_a_vault_holding_another() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        let tmp = tempfile::tempdir().unwrap();
        let (keychain, _kc) = mem_vault("keychain", Some(&k1));
        let (sidecar, sc) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "keychain" });
        assert_eq!(sc.lock().unwrap().as_deref(), Some(k2.as_str()));
    }

    #[tokio::test]
    async fn key_stays_in_config_toml_when_no_vault_can_hold_it() {
        struct ReadOnlyEmpty;
        impl KeyVault for ReadOnlyEmpty {
            fn name(&self) -> &'static str {
                "sidecar"
            }
            fn retrieve(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn store(&self, _s: &str) -> Result<()> {
                anyhow::bail!("read-only data dir")
            }
        }
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let store = KeyStore::from_vaults(vec![Box::new(ReadOnlyEmpty)]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());

        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k));
    }

    #[tokio::test]
    async fn key_stays_in_config_toml_while_a_column_holds_rows_it_cannot_decrypt() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        // Snapshot column holds only rows under a lost key.
        seed_snapshot(&db, &crypto::generate_secret()).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k);
        let (vault, _cell) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![vault]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());

        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "legacy-config"
            }
        );
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k));
    }

    #[tokio::test]
    async fn a_different_legacy_key_is_set_aside_and_the_key_in_use_kept() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let stale = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &stale);
        let (vault, _cell) = mem_vault("sidecar", Some(&k));
        let store = KeyStore::from_vaults(vec![vault]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(stale.clone());

        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "sidecar" });
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()));
        // C3-03: the stale key decrypts nothing — set aside, never dropped —
        // and the key in use (one vault copy) gets config.toml's copy.
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k.clone()));
        assert_eq!(side_file_keys(tmp.path()), vec![stale]);
        assert!(boot_report(tmp.path()).file_keeps_key);
    }

    fn side_file_keys(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(RETIRED_KEY_PREFIX)
            })
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| {
                t.parse::<toml::Table>()
                    .ok()?
                    .get("encryption_secret")?
                    .as_str()
                    .map(str::to_string)
            })
            .collect()
    }

    /// C3-01 — an invalid config.toml value never stops the key in use from
    /// being persisted, even when the only vault holds garbage too.
    #[tokio::test]
    async fn an_invalid_legacy_value_never_blocks_persisting_the_key_in_use() {
        for sidecar in [Some("not-a-key"), None] {
            let db = Database::open_in_memory().unwrap();
            let tmp = tempfile::tempdir().unwrap();
            let (vault, _cell) = mem_vault("sidecar", sidecar);
            let store = KeyStore::from_vaults(vec![vault]);
            let mut cfg = config::default_config();
            cfg.encryption_secret = Some("日本".repeat(16));
            config::retain_disk_key(tmp.path(), &"日本".repeat(16));
            let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
                .await
                .unwrap();
            assert_eq!(outcome, KeyOutcome::Minted);
            let key = cfg.encryption_secret.clone().unwrap();
            assert_eq!(
                config::retained_disk_key(tmp.path()),
                Some(key),
                "{sidecar:?}"
            );
            assert_eq!(
                side_file_keys(tmp.path()),
                vec!["日本".repeat(16)],
                "garbage kept aside"
            );
        }
    }

    /// C3-03 — after KRONN_REENCRYPT_FROM retires the config.toml key, the
    /// key in use gets the file copy and the retired one goes to a side file.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_retired_config_key_is_set_aside_and_the_key_in_use_gets_the_file_copy() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        config::retain_disk_key(tmp.path(), &k1);
        let (sidecar, _s) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k1.clone());
        std::env::set_var(
            ENV_REENCRYPT_FROM,
            crypto::key_fingerprint_hex(&k1).unwrap(),
        );
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(ENV_REENCRYPT_FROM);
        assert_eq!(outcome.unwrap(), KeyOutcome::Resolved { source: "sidecar" });
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k2));
        assert_eq!(side_file_keys(tmp.path()), vec![k1]);
    }

    /// C3-07 — no durable copy of a new key can be written: the boot stops
    /// before anything is encrypted under it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_new_key_without_any_durable_copy_stops_the_boot() {
        use std::os::unix::fs::PermissionsExt;
        struct FailStore;
        impl KeyVault for FailStore {
            fn name(&self) -> &'static str {
                "sidecar"
            }
            fn retrieve(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn store(&self, _s: &str) -> Result<()> {
                anyhow::bail!("disk full")
            }
        }
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ro = tmp.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
        let store = KeyStore::from_vaults(vec![Box::new(FailStore)]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let res = reconcile_with(&mut cfg, &db, &store, &ro).await;
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
        if std::fs::write(ro.join("probe"), "x").is_ok() && res.is_ok() {
            return; // running as root: the directory stays writable
        }
        let err = res.unwrap_err().to_string();
        assert!(err.contains("No durable copy"), "{err}");
        assert!(cfg.encryption_secret.is_none());
        assert!(!has_ciphertext(&db).await.unwrap());
    }

    /// C3-08 — a damaged keychain item gets its own hint, naming the item.
    #[test]
    fn a_corrupted_keychain_hint_names_the_item() {
        let hint = vault_hint("keychain", true);
        assert!(hint.contains("com.kronn.kronn") && hint.contains("encryption_secret_v1"));
        assert!(hint.contains("Keychain Access"));
    }

    /// C3-19 — the several-keys message gives the macOS app form.
    #[test]
    fn the_several_keys_message_gives_the_app_form() {
        let msg = KeyBootError::SeveralKeys {
            details: "x".into(),
            service: "s",
            account: "a",
        }
        .to_string();
        assert!(
            msg.contains("open --env KRONN_REENCRYPT_FROM=<fingerprint> -a Kronn"),
            "{msg}"
        );
    }

    /// C3-10 — a key still sitting in config.toml.backup rescues a locked boot.
    #[tokio::test]
    async fn a_key_in_a_config_backup_rescues_a_locked_boot() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml.backup.20261005T000000Z"),
            format!("encryption_secret = \"{k}\"\n[server]\nport = 1\n"),
        )
        .unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &KeyStore::from_vaults(vec![]), tmp.path())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "config-backup"
            }
        );
        assert_eq!(config::retained_disk_key(tmp.path()), Some(k));
    }

    /// C3-11 — after a runtime restore the status describes the restored key.
    #[tokio::test]
    async fn the_boot_report_follows_a_restore() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let (sidecar, _s) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(boot_report(tmp.path()).copies, 0);
        let code = recovery::to_code(&recovery::wrap_key(&k, "pw").unwrap());
        recover_with_passphrase(&mut cfg, &db, &store, "pw", Some(&code), tmp.path())
            .await
            .unwrap();
        assert_eq!(boot_report(tmp.path()).copies, 1);
    }

    /// C3-12 — with two imported keys, rewritten + already_current covers
    /// every row once.
    #[tokio::test]
    async fn reencrypt_totals_count_each_row_once() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let a = crypto::generate_secret();
        let b = crypto::generate_secret();
        seed_row(&db, &k).await;
        seed_snapshot(&db, &a).await;
        seed_credential(&db, &b).await;
        let tmp = tempfile::tempdir().unwrap();
        recovery::save_imported_blob(tmp.path(), &recovery::wrap_key(&a, "pass").unwrap()).unwrap();
        recovery::save_imported_blob(tmp.path(), &recovery::wrap_key(&b, "pass").unwrap()).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        let r = reencrypt_imported(&cfg, &db, "pass", None, tmp.path())
            .await
            .unwrap();
        assert_eq!(r.rewritten, 2);
        assert_eq!(r.rewritten + r.already_current + r.untouched, 3, "{r:?}");
        assert_eq!(r.already_current, 1);
    }

    /// C3-18 — a backup still under a retired key that a vault holds is moved
    /// under the key in use at the next start.
    #[tokio::test]
    async fn the_config_backup_follows_the_key_in_use_at_every_start() {
        let db = Database::open_in_memory().unwrap();
        let to = crypto::generate_secret();
        let from = crypto::generate_secret();
        seed_row(&db, &to).await;
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp
            .path()
            .join(crate::core::credential_store::BACKUP_FILENAME);
        std::fs::write(
            &backup,
            crypto::encrypt("old config", &crypto::parse_secret(&from).unwrap()).unwrap(),
        )
        .unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&to));
        let (sidecar, _b) = mem_vault("sidecar", Some(&from));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            crate::core::credential_store::read_backup(&backup, &to).unwrap(),
            "old config"
        );
    }

    // ── review round 5 ──────────────────────────────────────────────────────

    /// C4-01 — rows under a key that survives only in a config backup or a
    /// set-aside file are seen: two decrypting keys, SeveralKeys names it.
    #[tokio::test]
    async fn a_key_kept_only_in_a_file_that_decrypts_rows_is_seen() {
        // C5-01: rows under a key kept only in a file are moved under the live
        // store key at boot (checked, one transaction), and the file is kept.
        for file in [
            "config.toml.backup.20261005T000000Z",
            "config.toml.retired-key.20261005T000000Z",
            "config.toml.corrupt.20261005T000000Z",
        ] {
            let db = Database::open_in_memory().unwrap();
            let k = crypto::generate_secret();
            let k1 = crypto::generate_secret();
            seed_row(&db, &k).await;
            seed_snapshot(&db, &k1).await;
            let tmp = tempfile::tempdir().unwrap();
            let text = format!("encryption_secret = \"{k1}\"\nx = [broken\n");
            let text = if file.contains("corrupt") {
                text
            } else {
                format!("encryption_secret = \"{k1}\"\n")
            };
            std::fs::write(tmp.path().join(file), &text).unwrap();
            let (sidecar, _s) = mem_vault("sidecar", Some(&k));
            let store = KeyStore::from_vaults(vec![sidecar]);
            let mut cfg = config::default_config();
            cfg.encryption_secret = None;
            let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
                .await
                .unwrap();
            assert_eq!(
                outcome,
                KeyOutcome::Resolved { source: "sidecar" },
                "{file}"
            );
            let columns = collect_encrypted_rows(&db).await.unwrap();
            assert!(
                columns
                    .iter()
                    .all(|c| c.sample.iter().all(|enc| decrypts(enc, &k))),
                "{file}: every row under the store key"
            );
            assert_eq!(
                std::fs::read_to_string(tmp.path().join(file)).unwrap(),
                text
            );
            let moved = boot_report(tmp.path()).rows_moved_from_files;
            assert!(
                moved.len() == 1 && moved[0].contains(file),
                "{file}: {moved:?}"
            );
        }
    }

    /// C5-01 — two LIVE key stores that each decrypt rows still stop the boot,
    /// and the message names the kept files to keep.
    #[tokio::test]
    async fn two_live_keys_still_stop_and_name_the_kept_files() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        let k3 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml.backup.1"),
            format!("encryption_secret = \"{k3}\"\n"),
        )
        .unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&k1));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let err = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("keychain holds key") && err.contains("sidecar holds key"),
            "{err}"
        );
        assert!(err.contains("config.toml.backup*"), "{err}");
    }

    /// C5-02 — a key readable only in config.toml.corrupt.<ts> is tried: the
    /// boot resolves instead of locking.
    #[tokio::test]
    async fn a_key_only_in_a_corrupt_config_resolves_the_boot() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("config.toml.corrupt.20261005T000000Z"),
            format!("encryption_secret = \"{k}\"\nx = [broken\n"),
        )
        .unwrap();
        let (sidecar, _s) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            outcome,
            KeyOutcome::Resolved {
                source: "corrupt-config"
            }
        );
    }

    /// C4-02 — a recovery.key verified for ANOTHER key is replaced without its
    /// passphrase, the old blob kept as previous.
    #[test]
    fn a_recovery_key_for_another_key_is_replaced_without_its_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let k_old = crypto::generate_secret();
        let k = crypto::generate_secret();
        recovery::save_blob(
            dir.path(),
            &recovery::wrap_key(&k_old, "forgotten").unwrap(),
        )
        .unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        set_recovery_passphrase_in(dir.path(), &cfg, "new long pass", None).unwrap();
        assert_eq!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Matches
        );
        let kept = recovery::previous_blobs(dir.path());
        assert_eq!(recovery::unwrap_key(&kept[0], "forgotten").unwrap(), k_old);
        // A blob that matches (or cannot be verified) still needs the current one.
        assert!(set_recovery_passphrase_in(dir.path(), &cfg, "another long pass", None).is_err());
    }

    /// C4-03 — after a restore, a store still holding another value is reported.
    #[tokio::test]
    async fn the_report_after_a_restore_names_the_stale_store() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let w = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let (sidecar, _s) = mem_vault("sidecar", Some(&w));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        let code = recovery::to_code(&recovery::wrap_key(&k, "pw").unwrap());
        recover_with_passphrase(&mut cfg, &db, &store, "pw", Some(&code), tmp.path())
            .await
            .unwrap();
        assert_eq!(boot_report(tmp.path()).stale_sources, vec!["sidecar"]);
    }

    /// C4-05 — config.toml already holding the key is the durable copy: no
    /// rewrite, so a read-only data directory does not stop the boot.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_existing_config_copy_is_accepted_without_rewriting() {
        use std::os::unix::fs::PermissionsExt;
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        let text = format!("encryption_secret = \"{k}\"\n\n[server]\nport = 1\n");
        std::fs::write(dir.join("config.toml"), &text).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        let res = reconcile_with(&mut cfg, &db, &KeyStore::from_vaults(vec![]), &dir).await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        res.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            text
        );
    }

    /// C4-08 — a retirement whose backup move fails still boots (rows moved);
    /// the next start moves the backup.
    #[tokio::test]
    #[serial_test::serial]
    async fn a_failed_backup_move_during_retirement_does_not_stop_the_boot() {
        let db = Database::open_in_memory().unwrap();
        let k1 = crypto::generate_secret();
        let k2 = crypto::generate_secret();
        seed_row(&db, &k1).await;
        seed_snapshot(&db, &k2).await;
        let tmp = tempfile::tempdir().unwrap();
        let backup = tmp
            .path()
            .join(crate::core::credential_store::BACKUP_FILENAME);
        std::fs::write(
            &backup,
            crypto::encrypt("old config", &crypto::parse_secret(&k2).unwrap()).unwrap(),
        )
        .unwrap();
        let blocked = tmp.path().join(format!(
            ".{}.reencrypt.tmp",
            crate::core::credential_store::BACKUP_FILENAME
        ));
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("x"), "x").unwrap();
        let (keychain, _a) = mem_vault("keychain", Some(&k1));
        let (sidecar, _b) = mem_vault("sidecar", Some(&k2));
        let store = KeyStore::from_vaults(vec![keychain, sidecar]);
        std::env::set_var(
            ENV_REENCRYPT_FROM,
            crypto::key_fingerprint_hex(&k2).unwrap(),
        );
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let outcome = reconcile_with(&mut cfg, &db, &store, tmp.path()).await;
        std::env::remove_var(ENV_REENCRYPT_FROM);
        outcome.unwrap();
        assert!(decrypts_every_column(
            &k1,
            &collect_encrypted_rows(&db).await.unwrap()
        ));
        std::fs::remove_dir_all(&blocked).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        reconcile_with(&mut cfg, &db, &store, tmp.path())
            .await
            .unwrap();
        assert_eq!(
            crate::core::credential_store::read_backup(&backup, &k1).unwrap(),
            "old config"
        );
    }

    /// C5-03 — a restore sets aside the other value config.toml held before
    /// the restored key takes the file copy.
    #[tokio::test]
    async fn a_restore_sets_aside_the_other_config_value() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let w = crypto::generate_secret();
        let w_prime = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(
            dir.join("config.toml"),
            format!("encryption_secret = \"{w_prime}\"\n"),
        )
        .unwrap();
        config::retain_disk_key(dir, &w_prime);
        let (sidecar, _s) = mem_vault("sidecar", Some(&w));
        let store = KeyStore::from_vaults(vec![sidecar]);
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(w_prime.clone());
        assert!(matches!(
            reconcile_with(&mut cfg, &db, &store, dir).await.unwrap(),
            KeyOutcome::Locked { .. }
        ));
        config::retain_disk_key(dir, &w_prime);
        let code = recovery::to_code(&recovery::wrap_key(&k, "pw").unwrap());
        recover_with_passphrase(&mut cfg, &db, &store, "pw", Some(&code), dir)
            .await
            .unwrap();
        let kept: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(RETIRED_KEY_PREFIX))
            .collect();
        let held = kept.iter().any(|n| {
            std::fs::read_to_string(dir.join(n))
                .unwrap()
                .contains(&w_prime)
        });
        assert!(held, "{kept:?}");
        assert_eq!(
            config::read_disk_key(dir).unwrap().as_deref(),
            Some(k.as_str())
        );
        config::release_disk_key(dir);
    }

    /// C5-03 — when the other config.toml value cannot be set aside and no
    /// vault holds the key, the boot stops instead of overwriting it.
    #[tokio::test]
    async fn a_failed_set_aside_never_lets_the_key_overwrite_the_old_value() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        let w_prime = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let text = format!("encryption_secret = \"{w_prime}\"\n");
        std::fs::write(dir.join("config.toml"), &text).unwrap();
        std::fs::write(
            dir.join("config.toml.backup.1"),
            format!("encryption_secret = \"{k}\"\n"),
        )
        .unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(w_prime.clone());
        FAIL_SET_ASIDE.with(|f| f.set(true));
        let res = reconcile_with(&mut cfg, &db, &KeyStore::from_vaults(vec![]), dir).await;
        FAIL_SET_ASIDE.with(|f| f.set(false));
        assert!(res.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            text
        );
        config::release_disk_key(dir);
    }

    /// C5-07 — a recovery.key with a damaged payload is replaced without a
    /// passphrase and kept.
    #[test]
    fn a_damaged_recovery_key_is_replaced_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let k = crypto::generate_secret();
        let mut blob = recovery::wrap_key(&crypto::generate_secret(), "old").unwrap();
        blob.wrapped = blob.wrapped[..8].to_string();
        recovery::save_blob(dir.path(), &blob).unwrap();
        assert_eq!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Unreadable
        );
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        set_recovery_passphrase_in(dir.path(), &cfg, "new long pass", None).unwrap();
        assert_eq!(
            recovery::matches_key(dir.path(), &k),
            recovery::RecoveryMatch::Matches
        );
        assert_eq!(recovery::previous_blobs(dir.path()).len(), 1);
    }

    /// C5-07 — a recovery.key from before 0.14.3 is replaced after the
    /// confirmation; a restore with the old passphrase still works.
    #[tokio::test]
    async fn an_unverified_recovery_key_is_replaced_on_confirmation_and_still_restores() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let mut old = recovery::wrap_key(&k, "old passphrase").unwrap();
        old.fingerprint = None;
        recovery::save_blob(dir, &old).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(k.clone());
        assert!(set_recovery_passphrase_in(dir, &cfg, "new long pass", None).is_err());
        set_recovery_passphrase_with(dir, &cfg, "new long pass", None, true).unwrap();
        assert_eq!(recovery::previous_blobs(dir).len(), 1);
        // Locked instance: the old passphrase still opens its kept file.
        let mut locked = config::default_config();
        locked.encryption_secret = None;
        let (sidecar, _s) = mem_vault("sidecar", None);
        let store = KeyStore::from_vaults(vec![sidecar]);
        recover_with_passphrase(&mut locked, &db, &store, "old passphrase", None, dir)
            .await
            .unwrap();
        assert_eq!(locked.encryption_secret.as_deref(), Some(k.as_str()));
        config::release_disk_key(dir);
    }

    /// C4-10 — every key or credential file write syncs its directory.
    #[test]
    fn every_key_file_writer_syncs_its_directory() {
        use crate::core::keyvault::{SidecarFile, DIR_SYNCS};
        let dir = tempfile::tempdir().unwrap();
        let count = || DIR_SYNCS.with(|c| c.get());
        let before = count();
        SidecarFile::in_dir(dir.path())
            .store(&crypto::generate_secret())
            .unwrap();
        assert_eq!(count(), before + 1, "sidecar");
        recovery::save_blob(
            dir.path(),
            &recovery::wrap_key(&crypto::generate_secret(), "pw").unwrap(),
        )
        .unwrap();
        assert_eq!(count(), before + 2, "recovery.key");
        set_aside_retired_key(dir.path(), "old").unwrap();
        assert_eq!(count(), before + 3, "retired-key");
        // C5-10: the credential and migration backups go through it too.
        let d = dir.path();
        let key = crypto::generate_secret();
        let plain = format!(
            "encryption_secret = \"{key}\"\n[tokens]\n[[tokens.keys]]\nid = \"a\"\nname = \"a\"\nprovider = \"openai\"\nvalue = \"sk-x\"\nactive = true\n"
        );
        std::fs::write(d.join("config.toml"), &plain).unwrap();
        let step = |label: &str, n: usize| assert!(count() > n, "{label}");
        let n = count();
        crate::core::credential_store::back_up_config_if_it_holds_secrets(d, &key).unwrap();
        step("encrypted backup", n);
        let n = count();
        crate::core::credential_store::reencrypt_backup(d, &key, &crypto::generate_secret())
            .unwrap();
        step("reencrypt_backup", n);
        let n = count();
        crate::db::migrations::backup_config_without_credentials(d).unwrap();
        step("config.toml.backup", n);
        let n = count();
        crate::db::migrations::rotate_config_backup(d, &d.join("config.toml.backup")).unwrap();
        step("rotation", n);
        std::fs::write(d.join("config.toml.backup"), &plain).unwrap();
        let n = count();
        crate::core::credential_store::scrub_one_backup(d, &d.join("config.toml.backup"), &key)
            .unwrap();
        step("scrub", n);
    }

    /// C3-21 — a refused set (too short) leaves no recovery.previous-* copy.
    #[test]
    fn a_refused_recovery_set_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let k_old = crypto::generate_secret();
        recovery::save_blob(dir.path(), &recovery::wrap_key(&k_old, "old pass").unwrap()).unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(crypto::generate_secret());
        assert!(set_recovery_passphrase_in(dir.path(), &cfg, "short", Some("old pass")).is_err());
        cfg.encryption_secret = None;
        assert!(
            set_recovery_passphrase_in(dir.path(), &cfg, "long enough pass", Some("old pass"))
                .is_err()
        );
        assert!(recovery::previous_blobs(dir.path()).is_empty());
    }

    /// Every column whose name says it holds ciphertext must be registered, and
    /// every registered column must exist: a new encrypted column cannot be
    /// forgotten by the reconciler.
    #[tokio::test]
    async fn encrypted_column_registry_matches_the_schema() {
        let db = Database::open_in_memory().unwrap();
        let found = db
            .with_conn(|conn| {
                let mut tables = conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                )?;
                let names = tables
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                let mut found = std::collections::BTreeSet::new();
                for table in names {
                    let mut cols = conn.prepare(&format!("PRAGMA table_info({table})"))?;
                    for col in cols.query_map([], |r| r.get::<_, String>(1))? {
                        let col = col?;
                        let lower = col.to_lowercase();
                        if lower.contains("encrypted") || lower.contains("cipher") {
                            found.insert((table.clone(), col));
                        }
                    }
                }
                Ok(found)
            })
            .await
            .unwrap();
        let registered: std::collections::BTreeSet<(String, String)> = ENCRYPTED_COLUMNS
            .iter()
            .map(|c| (c.table.to_string(), c.column.to_string()))
            .collect();
        assert_eq!(
            found, registered,
            "register the new encrypted column in keystore::ENCRYPTED_COLUMNS"
        );
    }

    // ── recovery/set never silently replaces recovery.key ────────────────────

    #[test]
    fn recovery_set_first_time_needs_no_current_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        let k = crypto::generate_secret();
        cfg.encryption_secret = Some(k.clone());
        let code = set_recovery_passphrase_in(dir.path(), &cfg, "first passphrase", None).unwrap();
        let blob = recovery::from_code(&code).unwrap();
        assert_eq!(recovery::unwrap_key(&blob, "first passphrase").unwrap(), k);
    }

    #[test]
    fn recovery_set_refuses_to_replace_without_the_current_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(crypto::generate_secret());
        set_recovery_passphrase_in(dir.path(), &cfg, "first passphrase", None).unwrap();
        let before = std::fs::read(dir.path().join("recovery.key")).unwrap();

        let missing =
            set_recovery_passphrase_in(dir.path(), &cfg, "attacker passphrase", None).unwrap_err();
        assert!(
            missing.to_string().contains("enter the current one"),
            "{missing}"
        );
        let wrong = set_recovery_passphrase_in(
            dir.path(),
            &cfg,
            "attacker passphrase",
            Some("guessed wrong"),
        )
        .unwrap_err();
        assert!(wrong.to_string().contains("incorrect"), "{wrong}");
        assert_eq!(
            std::fs::read(dir.path().join("recovery.key")).unwrap(),
            before,
            "recovery.key must be untouched"
        );

        let code = set_recovery_passphrase_in(
            dir.path(),
            &cfg,
            "second passphrase",
            Some("first passphrase"),
        )
        .unwrap();
        let blob = recovery::load_blob(dir.path()).unwrap();
        assert_eq!(recovery::to_code(&blob), code);
        assert!(recovery::unwrap_key(&blob, "second passphrase").is_ok());
    }

    #[test]
    fn recovery_set_refuses_to_replace_an_unreadable_recovery_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("recovery.key"), "garbage").unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = Some(crypto::generate_secret());
        let err =
            set_recovery_passphrase_in(dir.path(), &cfg, "new passphrase!", Some("x")).unwrap_err();
        assert!(err.to_string().contains("cannot be read"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("recovery.key")).unwrap(),
            "garbage"
        );
    }

    #[tokio::test]
    async fn recovered_key_is_kept_in_config_toml_when_no_vault_takes_it() {
        let db = Database::open_in_memory().unwrap();
        let k = crypto::generate_secret();
        seed_row(&db, &k).await;
        let code = recovery::to_code(&recovery::wrap_key(&k, "pw").unwrap());
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![]);
        recover_with_passphrase(&mut cfg, &db, &store, "pw", Some(&code), dir.path())
            .await
            .unwrap();
        assert_eq!(config::retained_disk_key(dir.path()), Some(k));
    }
}
