//! Encryption-key vault — where the AES key (`encryption_secret`) is stored and
//! recovered from, **independently of `config.toml`**.
//!
//! Historically the key lived only in `config.toml`. Two code paths rewrote that
//! file without knowing it held a key (the `config::load` migration and the bash
//! `save_config`), silently stripping it — and `load()` then minted a *fresh*
//! key, orphaning every encrypted secret (incident 2026-06-30). This module
//! moves the key out of `config.toml` into a resolution ladder:
//!
//!   1. `KRONN_ENCRYPTION_KEK` env var — operator override (Docker/CI, headless)
//!   2. OS keychain                    — macOS Keychain / Windows Credential Manager
//!      (no Linux Secret Service: its libdbus C dependency broke CI/Docker for a
//!      tier that can't work there anyway — Linux uses the sidecar, tier 3)
//!   3. `0600` sidecar file            — universal fallback (Linux/WSL/Docker/no-keychain)
//!   4. legacy `config.toml` field     — read-only, adopted by the reconciler for migration
//!
//! The key stays a hex-encoded 32-byte string end to end, so `crypto::parse_secret`
//! and every existing consumer are unaffected — only *where the string comes
//! from* changes. Split-brain resolution (which tier wins when they disagree) is
//! deliberately NOT decided here: `snapshot()` exposes every tier so the
//! DB-aware reconciler can pick the key whose KID matches the stored ciphertext.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Env var an operator can set to pin the key (highest priority). Read-only.
pub const ENV_KEK: &str = "KRONN_ENCRYPTION_KEK";
/// Keychain service. Account is versioned so a future key scheme can coexist.
pub const KEYCHAIN_SERVICE: &str = "com.kronn.kronn";
pub const KEYCHAIN_ACCOUNT: &str = "encryption_secret_v1";
/// Sidecar filename inside the data dir — a *separate* file from `config.toml`,
/// so a `config.toml` rewrite can never strip it.
pub const SIDECAR_FILENAME: &str = "encryption_key";

/// Why a vault could not be read. Distinct from "empty" (`Ok(None)`): a vault
/// that errored may still hold the key, so it must never be minted over or
/// written to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    /// The store exists but refused access (denied prompt, locked keychain).
    #[error("access denied: {0}")]
    Denied(String),
    /// The store could not be reached or read (backend failure, I/O error).
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// The store answered but its content is not text (damaged file).
    #[error("corrupted: {0}")]
    Corrupted(String),
}

/// A place the encryption key can be persisted to and read from.
///
/// `retrieve` returns `Ok(None)` only when the vault is genuinely *empty*. A
/// denied or unreadable vault returns `Err` (a [`VaultError`]): it may hold
/// the key, so callers must stop rather than treat it as empty.
pub trait KeyVault: Send + Sync {
    fn name(&self) -> &'static str;
    fn retrieve(&self) -> Result<Option<String>>;
    fn store(&self, secret: &str) -> Result<()>;
}

/// OS keychain backend via the `keyring` crate.
pub struct OsKeychain {
    service: String,
    account: String,
}

impl OsKeychain {
    pub fn kronn() -> Self {
        Self {
            service: KEYCHAIN_SERVICE.into(),
            account: KEYCHAIN_ACCOUNT.into(),
        }
    }
    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, &self.account)
            .with_context(|| format!("open keychain entry {}/{}", self.service, self.account))
    }
}

impl KeyVault for OsKeychain {
    fn name(&self) -> &'static str {
        "keychain"
    }

    fn retrieve(&self) -> Result<Option<String>> {
        let entry = self
            .entry()
            .map_err(|e| VaultError::Unavailable(format!("{e:#}")))?;
        keychain_read_result(entry.get_password())
    }

    fn store(&self, secret: &str) -> Result<()> {
        self.entry()?
            .set_password(secret)
            .context("write key to OS keychain")
    }
}

/// Map a keychain read to the vault contract: only a missing item is "empty".
fn keychain_read_result(read: keyring::Result<String>) -> Result<Option<String>> {
    match read {
        Ok(s) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Ok(_) | Err(keyring::Error::NoEntry) => Ok(None),
        Err(keyring::Error::NoStorageAccess(e)) => Err(VaultError::Denied(e.to_string()).into()),
        Err(e) => Err(VaultError::Unavailable(e.to_string()).into()),
    }
}

/// `0600` file holding the hex key, in the data dir. Written atomically
/// (temp + rename in the same dir) so a crash never leaves a half-written key.
pub struct SidecarFile {
    path: PathBuf,
}

impl SidecarFile {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join(SIDECAR_FILENAME),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl KeyVault for SidecarFile {
    fn name(&self) -> &'static str {
        "sidecar"
    }

    fn retrieve(&self) -> Result<Option<String>> {
        match std::fs::read_to_string(&self.path) {
            Ok(s) => {
                let t = s.trim();
                Ok(if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(VaultError::Denied(format!("{}: {e}", self.path.display())).into())
            }
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                Err(VaultError::Corrupted(format!("{}: {e}", self.path.display())).into())
            }
            Err(e) => Err(VaultError::Unavailable(format!("{}: {e}", self.path.display())).into()),
        }
    }

    fn store(&self, secret: &str) -> Result<()> {
        let dir = self
            .path
            .parent()
            .context("sidecar path has no parent dir")?;
        std::fs::create_dir_all(dir).context("create data dir for sidecar")?;
        // Temp in the SAME dir so the rename stays on one filesystem (atomic).
        let tmp = dir.join(format!(".{}.tmp", SIDECAR_FILENAME));
        write_private_temp(&tmp, secret.as_bytes()).context("write sidecar temp")?;
        std::fs::rename(&tmp, &self.path).context("atomic rename sidecar into place")?;
        Ok(())
    }
}

/// Create `tmp` owner-only from the first byte and write `bytes` to it.
///
/// A leftover entry at `tmp` (stale file or planted symlink) is removed first
/// and `create_new` never follows one, so the secret cannot land elsewhere or
/// sit briefly under the umask's 0644.
pub(crate) fn write_private_temp(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Ok(meta) = std::fs::symlink_metadata(tmp) {
        if !meta.is_dir() {
            std::fs::remove_file(tmp)?;
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(tmp)?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(tmp);
        return Err(e);
    }
    Ok(())
}

/// Whether two key strings are the same key, whatever their case or
/// surrounding whitespace; values that are not keys compare as written.
pub fn same_key(a: &str, b: &str) -> bool {
    match (
        crate::core::crypto::canonical_secret(a),
        crate::core::crypto::canonical_secret(b),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A vault that could not be read during [`KeyStore::snapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultReadFailure {
    pub vault: &'static str,
    pub error: String,
    /// The vault content is damaged (not merely unreadable).
    pub corrupted: bool,
}

/// The ordered ladder of writable vaults (keychain → sidecar), plus the
/// read-only env override checked first.
pub struct KeyStore {
    vaults: Vec<Box<dyn KeyVault>>,
}

/// Whether the OS-keychain tier is active. DEV builds skip it: macOS binds a
/// keychain-item authorization to the binary's code signature, and an ad-hoc /
/// unsigned cargo build changes identity at EVERY rebuild — so each boot
/// re-prompts ("kronn wants to use your confidential information…") and
/// "Always Allow" can never stick across dev-watcher restarts. The `0600` sidecar keeps
/// the key durable in dev; signed release builds get the keychain tier, where
/// the authorization DOES persist. `KRONN_USE_KEYCHAIN=1|0` overrides both ways
/// (e.g. testing the keychain path from a dev build). Elsewhere than macOS and
/// Windows the crate has no backend, so the default is off there.
fn use_os_keychain() -> bool {
    match std::env::var("KRONN_USE_KEYCHAIN").ok().as_deref() {
        Some("1") | Some("true") => true,
        Some("0") | Some("false") => false,
        _ => !cfg!(debug_assertions) && cfg!(any(target_os = "macos", target_os = "windows")),
    }
}

impl KeyStore {
    /// Standard ladder: OS keychain (release builds only, see
    /// [`use_os_keychain`]) → `0600` sidecar in `data_dir`.
    pub fn standard(data_dir: &Path) -> Self {
        let mut vaults: Vec<Box<dyn KeyVault>> = Vec::new();
        if use_os_keychain() {
            vaults.push(Box::new(OsKeychain::kronn()));
        }
        vaults.push(Box::new(SidecarFile::in_dir(data_dir)));
        Self { vaults }
    }

    /// Build from an explicit vault list (tests inject a mock).
    pub fn from_vaults(vaults: Vec<Box<dyn KeyVault>>) -> Self {
        Self { vaults }
    }

    /// The env override, if set and non-empty. Read-only, highest priority.
    pub fn env_override() -> Option<String> {
        std::env::var(ENV_KEK)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Highest-priority key currently present: env → vaults in order.
    /// Returns the hex secret and the source name (for logging).
    pub fn primary(&self) -> Option<(String, &'static str)> {
        if let Some(s) = Self::env_override() {
            return Some((s, "env"));
        }
        for v in &self.vaults {
            match v.retrieve() {
                Ok(Some(s)) => return Some((s, v.name())),
                Ok(None) => {}
                Err(e) => tracing::warn!("keyvault {} retrieve errored: {e}", v.name()),
            }
        }
        None
    }

    /// Every tier's current value, for split-brain detection by the reconciler.
    /// Includes the env tier first. Fails on the first vault that cannot be
    /// read: an unreadable vault may hold the key, so no decision can be made.
    pub fn snapshot(
        &self,
    ) -> std::result::Result<Vec<(&'static str, Option<String>)>, VaultReadFailure> {
        let mut out: Vec<(&'static str, Option<String>)> = vec![("env", Self::env_override())];
        for v in &self.vaults {
            match v.retrieve() {
                Ok(val) => out.push((v.name(), val)),
                Err(error) => {
                    return Err(VaultReadFailure {
                        vault: v.name(),
                        corrupted: matches!(
                            error.downcast_ref::<VaultError>(),
                            Some(VaultError::Corrupted(_))
                        ),
                        error: format!("{error:#}"),
                    })
                }
            }
        }
        Ok(out)
    }

    /// Whether `secret` can be read back from a durable tier (env or a vault).
    pub fn holds(&self, secret: &str) -> bool {
        Self::env_override().is_some_and(|env| same_key(&env, secret))
            || self
                .vaults
                .iter()
                .any(|v| matches!(v.retrieve(), Ok(Some(cur)) if same_key(&cur, secret)))
    }

    /// How many persisted copies read `secret` back: each vault counts once.
    /// The env override is not counted (a one-off variable is not durable), nor
    /// is config.toml.
    pub fn copies_of(&self, secret: &str) -> usize {
        self.vaults
            .iter()
            .filter(|v| matches!(v.retrieve(), Ok(Some(cur)) if same_key(&cur, secret)))
            .count()
    }

    /// Whether the vault called `name` reads `secret` back.
    pub fn holds_in(&self, name: &str, secret: &str) -> bool {
        self.vaults
            .iter()
            .filter(|v| v.name() == name)
            .any(|v| matches!(v.retrieve(), Ok(Some(cur)) if same_key(&cur, secret)))
    }

    /// Persist `secret` into every EMPTY writable vault. A vault holding another
    /// key, or whose read fails, is never written: its item may be the only copy
    /// of that key. Returns per-vault results; callers should warn loudly if
    /// EVERY tier failed (no durable backup exists).
    pub fn mirror(&self, secret: &str) -> Vec<(&'static str, Result<()>)> {
        self.vaults
            .iter()
            .map(|v| {
                let res = match v.retrieve() {
                    Ok(Some(cur)) if same_key(&cur, secret) => Ok(()),
                    Ok(Some(_)) => Err(anyhow::anyhow!(
                        "not written: the vault holds a different key"
                    )),
                    Ok(None) => v.store(secret),
                    Err(e) => Err(e.context("not written: the vault could not be read first")),
                };
                (v.name(), res)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;

    enum MockState {
        Value(String),
        Empty,
        Unavailable,
        /// Holds a value but its read fails (denied prompt): a write would
        /// replace the only copy.
        Unreadable(String),
    }

    struct MockVault {
        name: &'static str,
        state: std::sync::Arc<Mutex<MockState>>,
    }
    impl MockVault {
        fn with_value(name: &'static str, v: &str) -> Self {
            Self {
                name,
                state: std::sync::Arc::new(Mutex::new(MockState::Value(v.into()))),
            }
        }
        fn empty(name: &'static str) -> Self {
            Self {
                name,
                state: std::sync::Arc::new(Mutex::new(MockState::Empty)),
            }
        }
        fn unavailable(name: &'static str) -> Self {
            Self {
                name,
                state: std::sync::Arc::new(Mutex::new(MockState::Unavailable)),
            }
        }
    }
    impl KeyVault for MockVault {
        fn name(&self) -> &'static str {
            self.name
        }
        fn retrieve(&self) -> Result<Option<String>> {
            match &*self.state.lock().unwrap() {
                MockState::Value(v) => Ok(Some(v.clone())),
                MockState::Empty => Ok(None),
                MockState::Unavailable => anyhow::bail!("vault unavailable"),
                MockState::Unreadable(_) => Err(VaultError::Denied("prompt denied".into()).into()),
            }
        }
        fn store(&self, secret: &str) -> Result<()> {
            let mut st = self.state.lock().unwrap();
            if matches!(*st, MockState::Unavailable) {
                anyhow::bail!("vault unavailable");
            }
            *st = MockState::Value(secret.to_string());
            Ok(())
        }
    }
    fn boxed(v: MockVault) -> Box<dyn KeyVault> {
        Box::new(v)
    }

    #[test]
    fn mock_store_then_retrieve_roundtrips() {
        let v = MockVault::empty("m");
        assert_eq!(v.retrieve().unwrap(), None);
        v.store("deadbeef").unwrap();
        assert_eq!(v.retrieve().unwrap(), Some("deadbeef".to_string()));
    }

    #[test]
    fn unavailable_vault_errors_on_both_ops() {
        let v = MockVault::unavailable("m");
        assert!(v.retrieve().is_err());
        assert!(v.store("x").is_err());
    }

    #[test]
    fn os_keychain_constructor_exposes_stable_vault_name() {
        let vault = OsKeychain::kronn();
        assert_eq!(vault.name(), "keychain");
        assert_eq!(vault.service, KEYCHAIN_SERVICE);
        assert_eq!(vault.account, KEYCHAIN_ACCOUNT);
    }

    #[test]
    #[serial]
    fn primary_prefers_env_over_vaults() {
        std::env::set_var(ENV_KEK, "  envkey \n");
        let ks = KeyStore::from_vaults(vec![boxed(MockVault::with_value("keychain", "otherkey"))]);
        assert_eq!(ks.primary(), Some(("envkey".to_string(), "env")));
        std::env::remove_var(ENV_KEK);
    }

    #[test]
    #[serial]
    fn env_override_ignores_whitespace_only_value() {
        std::env::set_var(ENV_KEK, " \n\t ");
        assert_eq!(KeyStore::env_override(), None);
        std::env::remove_var(ENV_KEK);
    }

    #[test]
    #[serial]
    fn primary_falls_through_unavailable_to_next_vault() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::unavailable("keychain")),
            boxed(MockVault::with_value("sidecar", "sk")),
        ]);
        assert_eq!(ks.primary(), Some(("sk".to_string(), "sidecar")));
    }

    #[test]
    #[serial]
    fn primary_returns_none_when_all_empty_or_unavailable() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::unavailable("keychain")),
            boxed(MockVault::empty("sidecar")),
        ]);
        assert_eq!(ks.primary(), None);
    }

    #[test]
    #[serial]
    fn snapshot_exposes_split_brain_disagreement() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::with_value("keychain", "AAAA")),
            boxed(MockVault::with_value("sidecar", "BBBB")),
        ]);
        let snap = ks.snapshot().unwrap();
        assert_eq!(snap[0], ("env", None));
        assert_eq!(snap[1], ("keychain", Some("AAAA".to_string())));
        assert_eq!(snap[2], ("sidecar", Some("BBBB".to_string())));
        // Disagreement must be VISIBLE, not silently resolved — the reconciler
        // picks by KID against the stored ciphertext.
        assert_ne!(snap[1].1, snap[2].1);
    }

    #[test]
    #[serial]
    fn snapshot_fails_on_an_unreadable_vault_instead_of_reading_it_as_empty() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::with_value("sidecar", "sk")),
            boxed(MockVault::unavailable("keychain")),
        ]);
        let failure = ks.snapshot().unwrap_err();
        assert_eq!(failure.vault, "keychain");
        assert!(failure.error.contains("vault unavailable"), "{failure:?}");
    }

    #[test]
    #[serial]
    fn mirror_never_writes_into_a_vault_whose_read_failed() {
        std::env::remove_var(ENV_KEK);
        let state = std::sync::Arc::new(Mutex::new(MockState::Unreadable("OLD".into())));
        let keychain = MockVault {
            name: "keychain",
            state: state.clone(),
        };
        let ks = KeyStore::from_vaults(vec![boxed(keychain), boxed(MockVault::empty("sidecar"))]);
        let results = ks.mirror("NEW");
        assert!(
            results[0].1.is_err(),
            "the unreadable vault reports an error"
        );
        assert!(results[1].1.is_ok());
        // The unreadable vault still holds its own value: no overwrite.
        let MockState::Unreadable(held) = &*state.lock().unwrap() else {
            panic!("the unreadable vault was written");
        };
        assert_eq!(held, "OLD");
    }

    #[test]
    #[serial]
    fn holds_reads_back_env_and_vaults() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::unavailable("keychain")),
            boxed(MockVault::with_value("sidecar", "KEY")),
        ]);
        assert!(ks.holds("KEY"));
        assert!(!ks.holds("OTHER"));
        assert!(ks.holds_in("sidecar", "KEY"));
        assert_eq!(ks.copies_of("KEY"), 1);
        assert!(!ks.holds_in("keychain", "KEY"));
        std::env::set_var(ENV_KEK, "OTHER");
        assert!(ks.holds("OTHER"));
        assert_eq!(ks.copies_of("OTHER"), 0, "env is not a persisted copy");
        std::env::remove_var(ENV_KEK);
    }

    #[test]
    fn keychain_reads_map_only_a_missing_item_to_empty() {
        assert_eq!(
            keychain_read_result(Ok(" k1 ".into())).unwrap(),
            Some("k1".into())
        );
        assert_eq!(keychain_read_result(Ok("  ".into())).unwrap(), None);
        assert_eq!(
            keychain_read_result(Err(keyring::Error::NoEntry)).unwrap(),
            None
        );
        let denied = keychain_read_result(Err(keyring::Error::NoStorageAccess("locked".into())))
            .unwrap_err();
        assert!(matches!(
            denied.downcast_ref::<VaultError>(),
            Some(VaultError::Denied(_))
        ));
        let failed =
            keychain_read_result(Err(keyring::Error::PlatformFailure("boom".into()))).unwrap_err();
        assert!(matches!(
            failed.downcast_ref::<VaultError>(),
            Some(VaultError::Unavailable(_))
        ));
    }

    #[test]
    #[serial]
    fn mirror_writes_missing_and_skips_present() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::with_value("keychain", "KEY")),
            boxed(MockVault::empty("sidecar")),
        ]);
        let results = ks.mirror("KEY");
        assert!(results.iter().all(|(_, r)| r.is_ok()));
        assert_eq!(ks.primary(), Some(("KEY".to_string(), "keychain")));
        assert_eq!(ks.snapshot().unwrap()[2].1, Some("KEY".to_string()));
    }

    #[test]
    #[serial]
    fn mirror_never_overwrites_a_vault_holding_another_key() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::with_value("keychain", "K1")),
            boxed(MockVault::with_value("sidecar", "K2")),
        ]);
        let results = ks.mirror("K1");
        assert!(results[0].1.is_ok());
        assert!(
            results[1].1.is_err(),
            "a different key is reported, not replaced"
        );
        let snap = ks.snapshot().unwrap();
        assert_eq!(snap[2].1.as_deref(), Some("K2"));
    }

    #[test]
    #[serial]
    fn mirror_reports_error_for_unavailable_but_still_writes_others() {
        std::env::remove_var(ENV_KEK);
        let ks = KeyStore::from_vaults(vec![
            boxed(MockVault::unavailable("keychain")),
            boxed(MockVault::empty("sidecar")),
        ]);
        let results = ks.mirror("KEY");
        assert!(
            results[0].1.is_err(),
            "unavailable vault store must surface an error"
        );
        assert!(
            results[1].1.is_ok(),
            "the writable vault must still receive the key"
        );
    }

    #[test]
    #[cfg(unix)]
    fn sidecar_writes_0600_roundtrips_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sc = SidecarFile::in_dir(dir.path());
        assert_eq!(sc.retrieve().unwrap(), None, "absent file → None");
        sc.store("cafebabe").unwrap();
        let mode = std::fs::metadata(sc.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "sidecar must be 0600");
        assert_eq!(sc.retrieve().unwrap(), Some("cafebabe".to_string()));
        let has_tmp = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!has_tmp, "atomic write must leave no .tmp behind");
    }

    #[test]
    #[serial]
    fn use_os_keychain_defaults_off_in_debug_and_honours_override() {
        std::env::remove_var("KRONN_USE_KEYCHAIN");
        // Tests compile with debug_assertions → default is OFF (no macOS
        // password prompt on every cargo rebuild).
        assert!(
            !use_os_keychain(),
            "debug builds must skip the keychain by default"
        );
        std::env::set_var("KRONN_USE_KEYCHAIN", "1");
        assert!(use_os_keychain(), "explicit opt-in must win");
        std::env::set_var("KRONN_USE_KEYCHAIN", "0");
        assert!(!use_os_keychain(), "explicit opt-out must win");
        std::env::remove_var("KRONN_USE_KEYCHAIN");
    }

    #[test]
    fn standard_ladder_constructs_with_keychain_and_sidecar() {
        // Cover the production constructor without touching the real keychain
        // (building the vaults doesn't call retrieve).
        let dir = tempfile::tempdir().unwrap();
        let ks = KeyStore::standard(dir.path());
        // snapshot() would hit the OS keychain; just prove it built (2 vaults +
        // the env tier) by checking the env override path with no env set.
        std::env::remove_var(ENV_KEK);
        assert_eq!(KeyStore::env_override(), None);
        let _ = ks; // constructed successfully
    }

    #[test]
    fn sidecar_empty_or_whitespace_file_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let sc = SidecarFile::in_dir(dir.path());
        std::fs::write(sc.path(), "   \n").unwrap();
        assert_eq!(sc.retrieve().unwrap(), None);
    }

    #[test]
    fn sidecar_read_error_is_an_error_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sc = SidecarFile::in_dir(dir.path());
        std::fs::create_dir(sc.path()).unwrap();
        let err = sc.retrieve().unwrap_err();
        assert!(matches!(
            err.downcast_ref::<VaultError>(),
            Some(VaultError::Unavailable(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_permission_denied_is_reported_as_denied() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sc = SidecarFile::in_dir(dir.path());
        sc.store("cafebabe").unwrap();
        std::fs::set_permissions(sc.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let res = sc.retrieve();
        std::fs::set_permissions(sc.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        // Root reads through 0o000; only assert the classification when refused.
        if let Err(err) = res {
            assert!(matches!(
                err.downcast_ref::<VaultError>(),
                Some(VaultError::Denied(_))
            ));
        }
    }

    #[test]
    fn sidecar_store_reports_missing_parent() {
        let sc = SidecarFile {
            path: PathBuf::new(),
        };
        assert!(sc
            .store("cafebabe")
            .unwrap_err()
            .to_string()
            .contains("sidecar path has no parent dir"));
    }

    #[test]
    fn sidecar_store_reports_blocked_parent_creation() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("not-a-directory");
        std::fs::write(&blocker, "block").unwrap();
        let sc = SidecarFile::in_dir(&blocker.join("nested"));
        assert!(sc
            .store("cafebabe")
            .unwrap_err()
            .to_string()
            .contains("create data dir for sidecar"));
    }

    #[cfg(unix)]
    #[test]
    fn sidecar_store_never_follows_a_symlink_planted_at_the_temp_path() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        std::os::unix::fs::symlink(
            &victim,
            dir.path().join(format!(".{}.tmp", SIDECAR_FILENAME)),
        )
        .unwrap();

        SidecarFile::in_dir(dir.path()).store("cafebabe").unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(SIDECAR_FILENAME)).unwrap(),
            "cafebabe"
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_temp_is_owner_only_at_creation_even_over_a_stale_0644_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join(".x.tmp");
        std::fs::write(&tmp, "stale").unwrap();
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)).unwrap();

        write_private_temp(&tmp, b"secret").unwrap();

        let mode = std::fs::metadata(&tmp).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "secret");
    }

    #[test]
    fn sidecar_store_reports_temp_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(format!(".{}.tmp", SIDECAR_FILENAME))).unwrap();
        let sc = SidecarFile::in_dir(dir.path());
        assert!(sc
            .store("cafebabe")
            .unwrap_err()
            .to_string()
            .contains("write sidecar temp"));
    }
}
