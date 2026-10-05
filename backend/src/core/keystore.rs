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
//! - Mirror the resolved key into every writable vault so it survives a
//!   `config.toml` rewrite / keychain reset. The `config.toml` copy is dropped
//!   only once a vault reads the key back and it decrypts every column.

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

/// Rows read per column (newest first) when testing keys. High enough to see
/// rows under a second key, bounded so a huge snapshot table cannot stall boot.
const SAMPLE_PER_COLUMN: usize = 10_000;

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
         Nothing was changed: Kronn will not pick one, the other's data would be lost. Back up \
         every copy (the OS keychain item {service}/{account}, the encryption_key file in the \
         data directory, KRONN_ENCRYPTION_KEK, config.toml), then remove the key whose data you \
         no longer need, or re-enter that data under the key you keep, and restart Kronn."
    )]
    SeveralKeys {
        details: String,
        service: &'static str,
        account: &'static str,
    },
}

fn vault_hint(vault: &str) -> &'static str {
    match vault {
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
    /// (source, key fingerprint, rows it decrypts) per decrypting candidate.
    Conflict(Vec<(&'static str, String, usize)>),
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
                    (*src, fp, *n)
                })
                .collect(),
        ),
    }
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
                "SELECT {} FROM {} WHERE {filter} ORDER BY rowid DESC LIMIT {SAMPLE_PER_COLUMN}",
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

/// Decide whether `config.toml` must keep a copy of the key. It is dropped only
/// when a durable tier reads `key` back and `key` decrypts every column; a
/// different legacy key is kept, since rows under it would be lost otherwise.
fn settle_disk_copy(
    dir: &Path,
    store: &KeyStore,
    key: &str,
    legacy: Option<&str>,
    columns: &[ColumnRows],
) {
    if let Some(old) = legacy.filter(|old| *old != key) {
        config::retain_disk_key(dir, old);
        tracing::warn!(
            "keystore: config.toml holds a different key than the one in use; it is kept there \
             until no encrypted row needs it"
        );
        return;
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
    let enough_copies =
        copies >= 2 || (copies >= 1 && recovery == recovery::RecoveryMatch::Matches);
    if enough_copies && decrypts_every_column(key, columns) {
        if config::release_disk_key(dir) {
            tracing::info!(
                "keystore: {copies} vault copies (recovery passphrase: {recovery:?}) and the key \
                 decrypts every encrypted column — config.toml no longer keeps a copy"
            );
        }
    } else {
        config::retain_disk_key(dir, key);
        tracing::warn!(
            "keystore: fewer than two independent copies of the key (vaults, verified recovery \
             passphrase), or a column holds rows it cannot decrypt — config.toml keeps a copy so \
             the key is not lost"
        );
    }
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
            hint: vault_hint(failure.vault),
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
    let legacy = config.encryption_secret.clone().filter(|k| !k.is_empty());
    if let Some(legacy) = legacy.clone() {
        candidates.push((legacy, "legacy-config"));
    }
    // De-dup by value (a key present in several tiers is one candidate).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(v, _)| seen.insert(v.clone()));

    let columns = collect_encrypted_rows(db).await?;

    let (key, outcome) = match decide(&candidates, &columns) {
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
                .map(|(src, fp, n)| format!("{src} holds key {fp} (decrypts {n} row(s))"))
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
                 them. Booting in a LOCKED state — restore the correct key (keychain / sidecar \
                 / KRONN_ENCRYPTION_KEK / recovery passphrase) or re-enter the secrets. \
                 Nothing was overwritten.",
                names.join(", ")
            );
            config.encryption_secret = None;
            return Ok(KeyOutcome::Locked { encrypted_rows: n });
        }
    };
    config.encryption_secret = Some(key.clone());
    persist(store, &key);
    settle_disk_copy(dir, store, &key, legacy.as_deref(), &columns);
    Ok(outcome)
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
) -> Result<String> {
    let dir = config::config_dir()?;
    set_recovery_passphrase_in(&dir, config, passphrase, current_passphrase)
}

pub(crate) fn set_recovery_passphrase_in(
    dir: &Path,
    config: &AppConfig,
    passphrase: &str,
    current_passphrase: Option<&str>,
) -> Result<String> {
    if recovery::is_configured(dir) {
        let existing = recovery::load_blob(dir).ok_or_else(|| {
            anyhow::anyhow!(
                "the existing recovery.key cannot be read, so it cannot be replaced from here — \
                 move it out of the Kronn data directory first if you no longer need it"
            )
        })?;
        let current = current_passphrase.unwrap_or_default();
        if current.is_empty() {
            anyhow::bail!(
                "A recovery passphrase is already set: enter the current one to replace it"
            );
        }
        if recovery::unwrap_key(&existing, current).is_err() {
            anyhow::bail!("The current recovery passphrase is incorrect — nothing was changed");
        }
    }
    if passphrase.chars().count() < MIN_RECOVERY_PASSPHRASE_LEN {
        anyhow::bail!(
            "Recovery passphrase must be at least {MIN_RECOVERY_PASSPHRASE_LEN} characters \
             (a few words work well)"
        );
    }
    let key_hex = config.encryption_secret.clone().ok_or_else(|| {
        anyhow::anyhow!("no active encryption key to protect (token subsystem is locked?)")
    })?;
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
    let blob = match recovery_code {
        Some(code) if !code.trim().is_empty() => {
            recovery::from_code(code).map_err(|e| anyhow::anyhow!(e))?
        }
        _ => recovery::load_blob(dir).ok_or_else(|| {
            anyhow::anyhow!("no recovery data — provide the recovery code you saved")
        })?,
    };

    let key = recovery::unwrap_key(&blob, passphrase).map_err(|e| anyhow::anyhow!(e))?;

    // The recovered key must match THIS instance's data (unless there's none).
    let columns = collect_encrypted_rows(db).await?;
    if !columns.is_empty()
        && !columns
            .iter()
            .any(|col| col.sample.iter().any(|enc| decrypts(enc, &key)))
    {
        return Err(anyhow::anyhow!(
            "the recovered key does not decrypt this instance's data — wrong recovery code for this Kronn"
        ));
    }

    config.encryption_secret = Some(key.clone());
    persist(store, &key);
    if store.copies_of(&key) < 2
        && recovery::matches_key(dir, &key) != recovery::RecoveryMatch::Matches
    {
        // Fewer than two copies would survive a restart: keep one in config.toml.
        config::retain_disk_key(dir, &key);
    }
    tracing::info!("keystore: encryption key restored from recovery passphrase");
    Ok(KeyOutcome::Resolved {
        source: "recovery-passphrase",
    })
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
    async fn without_a_recovery_passphrase_config_toml_keeps_the_key_unless_the_sidecar_holds_it() {
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
    async fn a_different_legacy_key_is_kept_in_config_toml() {
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
        assert_eq!(config::retained_disk_key(tmp.path()), Some(stale));
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
