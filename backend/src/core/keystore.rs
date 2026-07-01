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
//! - Mirror the resolved key into every writable vault so it survives a
//!   `config.toml` rewrite / keychain reset.

use anyhow::{Context, Result};

use crate::core::{config, crypto, keyvault::KeyStore, recovery};
use crate::db::{mcps, Database};
use crate::models::AppConfig;

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
}

fn decide(candidates: &[(String, &'static str)], encrypted_rows: &[String]) -> Decision {
    if encrypted_rows.is_empty() {
        return match candidates.first() {
            Some((k, s)) => Decision::Adopt(k.clone(), s),
            None => Decision::Mint,
        };
    }
    // Authority = decrypt self-test. Accept the highest-priority candidate that
    // actually decrypts existing ciphertext; never accept a key by provenance.
    for (cand, src) in candidates {
        if encrypted_rows.iter().any(|enc| mcps::decrypt_env(enc, cand).is_ok()) {
            return Decision::Accept(cand.clone(), src);
        }
    }
    Decision::Lock(encrypted_rows.len())
}

/// Collect the non-empty `env_encrypted` blobs that actually hold secrets.
async fn collect_encrypted_rows(db: &Database) -> Result<Vec<String>> {
    db.with_conn(|conn| {
        let cfgs = mcps::list_configs(conn)?;
        Ok(cfgs
            .into_iter()
            .filter(|c| !c.env_encrypted.is_empty() && !c.env_keys.is_empty())
            .map(|c| c.env_encrypted)
            .collect())
    })
    .await
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
pub async fn reconcile_with(
    config: &mut AppConfig,
    db: &Database,
    store: &KeyStore,
) -> Result<KeyOutcome> {
    // Candidate keys, highest priority first: env → keychain → sidecar (the
    // vault ladder), then the legacy config.toml field.
    let mut candidates: Vec<(String, &'static str)> = Vec::new();
    for (name, val) in store.snapshot() {
        if let Some(v) = val {
            if !v.is_empty() {
                candidates.push((v, name));
            }
        }
    }
    if let Some(legacy) = config.encryption_secret.clone() {
        if !legacy.is_empty() {
            candidates.push((legacy, "legacy-config"));
        }
    }
    // De-dup by value (a key present in several tiers is one candidate).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|(v, _)| seen.insert(v.clone()));

    let rows = collect_encrypted_rows(db).await?;

    match decide(&candidates, &rows) {
        Decision::Mint => {
            let key = crypto::generate_secret();
            config.encryption_secret = Some(key.clone());
            persist(store, &key);
            tracing::info!("keystore: fresh install — minted a new encryption key");
            Ok(KeyOutcome::Minted)
        }
        Decision::Adopt(key, source) => {
            config.encryption_secret = Some(key.clone());
            persist(store, &key);
            tracing::info!("keystore: adopted existing key from {source} (no encrypted data yet)");
            Ok(KeyOutcome::Resolved { source })
        }
        Decision::Accept(key, source) => {
            config.encryption_secret = Some(key.clone());
            persist(store, &key);
            tracing::info!("keystore: key from {source} decrypts existing data — accepted");
            Ok(KeyOutcome::Resolved { source })
        }
        Decision::Lock(n) => {
            // I1 + I3: do NOT mint, do NOT touch ciphertext. Boot continues; the
            // token subsystem surfaces per-row decrypt failures as a locked state.
            tracing::error!(
                "keystore: {n} encrypted MCP config(s) present but NO available key decrypts \
                 them. Booting in a LOCKED state — restore the correct key (keychain / sidecar \
                 / KRONN_ENCRYPTION_KEK) or re-enter the secrets. Nothing was overwritten."
            );
            Ok(KeyOutcome::Locked { encrypted_rows: n })
        }
    }
}

/// Production entry point: reconcile against the standard vault ladder for the
/// current data dir. Call once at boot, right after `Database::open`.
pub async fn reconcile(config: &mut AppConfig, db: &Database) -> Result<KeyOutcome> {
    let dir = config::config_dir()?;
    let store = KeyStore::standard(&dir);
    reconcile_with(config, db, &store).await
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
pub fn set_recovery_passphrase(config: &AppConfig, passphrase: &str) -> Result<String> {
    if passphrase.chars().count() < MIN_RECOVERY_PASSPHRASE_LEN {
        anyhow::bail!(
            "Recovery passphrase must be at least {MIN_RECOVERY_PASSPHRASE_LEN} characters \
             (a few words work well)"
        );
    }
    let key_hex = config
        .encryption_secret
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no active encryption key to protect (token subsystem is locked?)"))?;
    let blob = recovery::wrap_key(&key_hex, passphrase).map_err(|e| anyhow::anyhow!(e))?;
    let dir = config::config_dir()?;
    recovery::save_blob(&dir, &blob).context("persist recovery sidecar")?;
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
        _ => recovery::load_blob(dir)
            .ok_or_else(|| anyhow::anyhow!("no recovery data — provide the recovery code you saved"))?,
    };

    let key = recovery::unwrap_key(&blob, passphrase).map_err(|e| anyhow::anyhow!(e))?;

    // The recovered key must match THIS instance's data (unless there's none).
    let rows = collect_encrypted_rows(db).await?;
    if !rows.is_empty() && !rows.iter().any(|enc| mcps::decrypt_env(enc, &key).is_ok()) {
        return Err(anyhow::anyhow!(
            "the recovered key does not decrypt this instance's data — wrong recovery code for this Kronn"
        ));
    }

    config.encryption_secret = Some(key.clone());
    persist(store, &key);
    tracing::info!("keystore: encryption key restored from recovery passphrase");
    Ok(KeyOutcome::Resolved { source: "recovery-passphrase" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keyvault::KeyVault;
    use std::collections::HashMap;

    /// A vault that always holds one fixed value — enough to drive `reconcile_with`.
    struct FixedVault {
        name: &'static str,
        val: String,
    }
    impl KeyVault for FixedVault {
        fn name(&self) -> &'static str { self.name }
        fn retrieve(&self) -> Result<Option<String>> { Ok(Some(self.val.clone())) }
        fn store(&self, _secret: &str) -> Result<()> { Ok(()) }
    }

    fn encrypt_row(secret: &str, k: &str, v: &str) -> String {
        let mut env = HashMap::new();
        env.insert(k.to_string(), v.to_string());
        mcps::encrypt_env(&env, secret).unwrap()
    }

    // ── decide(): the pure incident-proof core ──────────────────────────────

    #[test]
    fn empty_db_no_candidate_mints() {
        assert!(matches!(decide(&[], &[]), Decision::Mint));
    }

    #[test]
    fn empty_db_adopts_first_candidate() {
        match decide(&[("KKKK".into(), "keychain")], &[]) {
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
        match decide(&[(k.clone(), "sidecar")], &rows) {
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
        match decide(&cands, &rows) {
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
        match decide(&[(wrong, "legacy-config")], &rows) {
            Decision::Lock(n) => assert_eq!(n, 1),
            other => panic!("expected Lock, got {other:?}"),
        }
        assert_eq!(rows[0], enc, "decide must never mutate ciphertext");
    }

    #[test]
    fn rows_exist_and_no_candidate_at_all_locks() {
        let k = crypto::generate_secret();
        let rows = vec![encrypt_row(&k, "T", "v")];
        assert!(matches!(decide(&[], &rows), Decision::Lock(1)));
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

        let outcome = reconcile_with(&mut cfg, &db, &store).await.unwrap();
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
        assert_eq!(after, enc, "reconcile must not rewrite ciphertext (orphans == 0)");
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

        let outcome = reconcile_with(&mut cfg, &db, &store).await.unwrap();
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
            fn name(&self) -> &'static str { "failstore" }
            fn retrieve(&self) -> Result<Option<String>> { Ok(None) }
            fn store(&self, _s: &str) -> Result<()> { anyhow::bail!("no writable backup medium") }
        }
        let db = Database::open_in_memory().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        let store = KeyStore::from_vaults(vec![Box::new(FailStore)]);

        let outcome = reconcile_with(&mut cfg, &db, &store).await.unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
        assert!(cfg.encryption_secret.is_some(), "mint sets the key even if backup fails");
    }

    #[tokio::test]
    async fn reconcile_mints_on_a_truly_empty_db() {
        let db = Database::open_in_memory().unwrap();
        let mut cfg = config::default_config();
        cfg.encryption_secret = None;
        // No vault holds a key, no rows exist → mint.
        let store = KeyStore::from_vaults(vec![]);
        let outcome = reconcile_with(&mut cfg, &db, &store).await.unwrap();
        assert_eq!(outcome, KeyOutcome::Minted);
        assert!(cfg.encryption_secret.is_some(), "a fresh key must be set on an empty install");
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

        let outcome = reconcile_with(&mut cfg, &db, &store).await.unwrap();
        assert_eq!(outcome, KeyOutcome::Locked { encrypted_rows: 1 });
        // Fail-soft: the (wrong) config key is left as-is, ciphertext untouched.
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
            conn.execute("INSERT INTO mcp_servers (id, name, transport) VALUES ('s1','t','stdio')", [])?;
            conn.execute(
                "INSERT INTO mcp_configs (id, server_id, label, env_encrypted, env_keys_json) \
                 VALUES ('c1','s1','t', ?1, '[\"TOKEN\"]')",
                [enc],
            )?;
            Ok(())
        }).await.unwrap();
    }

    #[test]
    fn set_recovery_passphrase_enforces_min_length() {
        // The blob travels in exports → the passphrase is the only barrier
        // against offline brute-force. Backend must enforce, not just the UI.
        let cfg = config::default_config();
        let err = set_recovery_passphrase(&cfg, "short-pass").unwrap_err();
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

        let outcome = recover_with_passphrase(
            &mut cfg, &db, &store, "my-pass", Some(&code), dir.path()).await.unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "recovery-passphrase" });
        assert_eq!(cfg.encryption_secret.as_deref(), Some(k.as_str()), "key restored live");
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

        let outcome = recover_with_passphrase(
            &mut cfg, &db, &store, "pw", None, dir.path()).await.unwrap();
        assert_eq!(outcome, KeyOutcome::Resolved { source: "recovery-passphrase" });
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

        let res = recover_with_passphrase(
            &mut cfg, &db, &store, "wrong", Some(&code), dir.path()).await;
        assert!(res.is_err(), "wrong passphrase must not recover");
        assert!(cfg.encryption_secret.is_none(), "no key set on failed recovery");
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

        let res = recover_with_passphrase(
            &mut cfg, &db, &store, "pw", Some(&code), dir.path()).await;
        assert!(res.is_err(), "a key that can't decrypt this instance's data must be refused");
    }
}
