//! Recovery passphrase — the last-resort way to get the encryption key back.
//!
//! P0/P1 keep the key in the OS keychain + a `0600` sidecar and never orphan it.
//! But every one of those lives ON the machine: an OS reinstall, a wiped
//! keychain, or a lost data dir takes them all. This module adds a THIRD,
//! off-machine-capable recovery path — a user passphrase.
//!
//! A KEK is derived from the passphrase with **Argon2id** (pinned params +
//! random salt), and the current 32-byte encryption key is AES-256-GCM–wrapped
//! under it into a [`RecoveryBlob`]. The blob is safe to store anywhere — a
//! sidecar file, an export, a printed "recovery code" — because without the
//! passphrase it's just ciphertext. To recover: derive the KEK from the same
//! passphrase + salt, unwrap, and you have the key back. This is exactly the
//! scenario that made the 2026-06-30 WSL tokens unrecoverable.
//!
//! Not an envelope refactor: the wrapped payload is the SAME flat key the rest
//! of the system already uses, so nothing downstream changes.

use std::path::Path;

use aes_gcm::aead::{rand_core::RngCore, OsRng};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use zeroize::Zeroize;

use crate::core::crypto;

const SALT_LEN: usize = 16;
/// Sidecar file (in the data dir) holding the recovery code. A local copy so a
/// key loss that leaves the data dir intact (config clobber, keychain reset) is
/// recoverable with just the passphrase — while the downloadable code covers
/// full data-dir loss. `0600`, never contains plaintext key material.
pub const RECOVERY_FILENAME: &str = "recovery.key";
/// Recovery-code prefix + format version. Bump if the KDF params or framing
/// change so old codes are detected rather than silently mis-derived.
const CODE_PREFIX: &str = "KRECOV1";

/// A passphrase-wrapped encryption key. `wrapped` is `crypto::encrypt`'s framing
/// (base64 of nonce‖ciphertext‖tag) of the key's hex string, under the
/// Argon2id-derived KEK. Holds no plaintext key material.
#[derive(Debug, Clone, PartialEq)]
pub struct RecoveryBlob {
    pub salt: [u8; SALT_LEN],
    pub wrapped: String,
    /// `crypto::key_fingerprint_hex` of the wrapped key, in clear, so the boot
    /// can tell whether this blob recovers the key in use without the
    /// passphrase. `None` for blobs written before 0.14.3 (unverified).
    pub fingerprint: Option<String>,
}

/// How the local `recovery.key` relates to a given key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryMatch {
    Absent,
    /// Present but not parseable.
    Unreadable,
    /// An older blob without a fingerprint: it may or may not wrap the key.
    Unverified,
    Matches,
    /// Wraps another key: it cannot restore this one.
    OtherKey,
}

/// Whether `recovery.key` in `dir` is known to recover `key_hex`.
pub fn matches_key(dir: &Path, key_hex: &str) -> RecoveryMatch {
    if !is_configured(dir) {
        return RecoveryMatch::Absent;
    }
    let Some(blob) = load_blob(dir) else {
        return RecoveryMatch::Unreadable;
    };
    if B64.decode(&blob.wrapped).map(|w| w.len()).ok() != Some(WRAPPED_LEN) {
        return RecoveryMatch::Unreadable;
    }
    match (blob.fingerprint, crypto::key_fingerprint_hex(key_hex)) {
        (None, _) => RecoveryMatch::Unverified,
        (Some(fp), Ok(active)) if fp.eq_ignore_ascii_case(&active) => RecoveryMatch::Matches,
        _ => RecoveryMatch::OtherKey,
    }
}

/// Argon2id with PINNED params (not `Argon2::default()`, whose defaults could
/// drift across crate versions and break existing codes). 19 MiB / 2 passes / 1
/// lane / 32-byte output — the OWASP baseline, fast enough on modest hardware
/// (WSL/Docker) for an infrequent boot/recovery op.
fn kdf() -> argon2::Argon2<'static> {
    let params =
        argon2::Params::new(19_456, 2, 1, Some(32)).expect("pinned Argon2 params are valid");
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
}

fn derive_kek(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut kek = [0u8; 32];
    kdf()
        .hash_password_into(passphrase.as_bytes(), salt, &mut kek)
        .map_err(|e| format!("Argon2 key derivation failed: {e}"))?;
    Ok(kek)
}

/// Wrap `key_hex` (a 64-char hex encryption key) under `passphrase`.
pub fn wrap_key(key_hex: &str, passphrase: &str) -> Result<RecoveryBlob, String> {
    if passphrase.is_empty() {
        return Err("Recovery passphrase must not be empty".into());
    }
    // Refuse to wrap garbage — the input must be a real key.
    crypto::parse_secret(key_hex)?;

    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);
    let mut kek = derive_kek(passphrase, &salt)?;
    let wrapped = crypto::encrypt(key_hex, &kek);
    kek.zeroize();
    Ok(RecoveryBlob {
        salt,
        wrapped: wrapped?,
        fingerprint: Some(crypto::key_fingerprint_hex(key_hex)?),
    })
}

/// Recover the key hex from a blob + passphrase. A wrong passphrase (or a
/// tampered blob) fails at the AES-GCM tag — never returns a wrong key.
pub fn unwrap_key(blob: &RecoveryBlob, passphrase: &str) -> Result<String, String> {
    let mut kek = derive_kek(passphrase, &blob.salt)?;
    let result = crypto::decrypt(&blob.wrapped, &kek);
    kek.zeroize();
    let key_hex =
        result.map_err(|_| "Wrong recovery passphrase or corrupt recovery data".to_string())?;
    // The unwrapped value must itself be a valid key (defense in depth), in
    // its one canonical spelling so comparisons with the active key hold.
    crypto::canonical_secret(&key_hex)
}

/// Serialize a blob to a portable "recovery code" string the user can save
/// off-machine. Safe to expose — useless without the passphrase.
pub fn to_code(blob: &RecoveryBlob) -> String {
    // `wrapped` is standard base64 (no '.'), so '.' is an unambiguous delimiter.
    let base = format!("{}.{}.{}", CODE_PREFIX, B64.encode(blob.salt), blob.wrapped);
    match &blob.fingerprint {
        Some(fp) => format!("{base}.{fp}.{}", checksum(blob, fp)),
        None => base,
    }
}

/// Binds the clear fingerprint to the wrapped payload: a damaged payload with
/// an intact fingerprint must not count as a verified copy.
fn checksum(blob: &RecoveryBlob, fingerprint: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"kronn-recovery-v1");
    hasher.update(blob.salt);
    hasher.update(blob.wrapped.as_bytes());
    hasher.update(fingerprint.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Length of `wrapped` once decoded: nonce (12) + the 64-char key + tag (16).
const WRAPPED_LEN: usize = 12 + 64 + 16;

/// Parse a recovery code back into a blob.
pub fn from_code(code: &str) -> Result<RecoveryBlob, String> {
    let parts: Vec<&str> = code.trim().split('.').collect();
    if !(3..=5).contains(&parts.len()) || parts[0] != CODE_PREFIX {
        return Err("Invalid recovery code format".into());
    }
    let salt_bytes = B64
        .decode(parts[1])
        .map_err(|e| format!("Invalid recovery code salt: {e}"))?;
    if salt_bytes.len() != SALT_LEN {
        return Err(format!(
            "Invalid recovery code salt length: {}",
            salt_bytes.len()
        ));
    }
    if parts[2].is_empty() {
        return Err("Invalid recovery code: empty payload".into());
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&salt_bytes);
    let is_hex16 = |s: &str| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit());
    let mut blob = RecoveryBlob {
        salt,
        wrapped: parts[2].to_string(),
        fingerprint: None,
    };
    match (parts.get(3), parts.get(4)) {
        (None, _) => {}
        (Some(fp), Some(sum)) if is_hex16(fp) && is_hex16(sum) => {
            if !checksum(&blob, fp).eq_ignore_ascii_case(sum) {
                return Err("Corrupt recovery code: checksum mismatch".into());
            }
            blob.fingerprint = Some(fp.to_string());
        }
        // A fingerprint without its checksum is not trusted: unverified.
        (Some(fp), None) if is_hex16(fp) => {}
        _ => return Err("Invalid recovery code fingerprint".into()),
    }
    Ok(blob)
}

/// Persist the recovery code to the `0600` sidecar in `dir` (atomic temp+rename
/// in the same dir, mirroring `keyvault::SidecarFile`).
pub fn save_blob(dir: &Path, blob: &RecoveryBlob) -> std::io::Result<()> {
    save_blob_as(dir, RECOVERY_FILENAME, blob)
}

/// Like [`save_blob`] under another file name (imported blobs never replace
/// this machine's `recovery.key`).
pub fn save_blob_as(dir: &Path, filename: &str, blob: &RecoveryBlob) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(filename);
    // One temp per write: two concurrent writers never share a half-written file.
    let tmp = dir.join(format!(".{filename}.{}.{seq}.tmp", std::process::id()));
    crate::core::keyvault::write_private_atomic(&tmp, &path, to_code(blob).as_bytes())
}

/// File-name prefix of blobs carried by imported backups (another machine's key).
pub const IMPORTED_PREFIX: &str = "recovery.imported-";

/// Store an imported blob under a new timestamped name; never replaces
/// `recovery.key` nor an earlier import. Returns the file name.
pub fn save_imported_blob(dir: &Path, blob: &RecoveryBlob) -> std::io::Result<String> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
    let mut name = format!("{IMPORTED_PREFIX}{stamp}.key");
    let mut n = 1;
    while dir.join(&name).exists() {
        name = format!("{IMPORTED_PREFIX}{stamp}-{n}.key");
        n += 1;
    }
    save_blob_as(dir, &name, blob)?;
    Ok(name)
}

/// File-name prefix of a replaced `recovery.key` that wrapped another key.
pub const PREVIOUS_PREFIX: &str = "recovery.previous-";

/// Keep a replaced `recovery.key` that wraps another key than the active one.
pub fn save_previous_blob(dir: &Path, blob: &RecoveryBlob) -> std::io::Result<String> {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ");
    let mut name = format!("{PREVIOUS_PREFIX}{stamp}.key");
    let mut n = 1;
    while dir.join(&name).exists() {
        name = format!("{PREVIOUS_PREFIX}{stamp}-{n}.key");
        n += 1;
    }
    save_blob_as(dir, &name, blob)?;
    Ok(name)
}

/// Kept blobs of a replaced `recovery.key`, newest first.
pub fn previous_blobs(dir: &Path) -> Vec<RecoveryBlob> {
    blobs_with_prefix(dir, PREVIOUS_PREFIX)
}

/// Imported blobs, newest first.
pub fn imported_blobs(dir: &Path) -> Vec<RecoveryBlob> {
    blobs_with_prefix(dir, IMPORTED_PREFIX)
}

fn blobs_with_prefix(dir: &Path, prefix: &str) -> Vec<RecoveryBlob> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(prefix) && n.ends_with(".key"))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.reverse();
    names
        .into_iter()
        .filter_map(|n| std::fs::read_to_string(dir.join(n)).ok())
        .filter_map(|code| from_code(&code).ok())
        .collect()
}

/// Load the recovery blob from the sidecar, or `None` if absent/unreadable.
pub fn load_blob(dir: &Path) -> Option<RecoveryBlob> {
    let code = std::fs::read_to_string(dir.join(RECOVERY_FILENAME)).ok()?;
    from_code(&code).ok()
}

/// Is a recovery passphrase configured (sidecar present)?
pub fn is_configured(dir: &Path) -> bool {
    dir.join(RECOVERY_FILENAME).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_key() -> String {
        crypto::generate_secret()
    }

    #[test]
    fn wrap_then_unwrap_roundtrips() {
        let key = a_key();
        let blob = wrap_key(&key, "correct horse battery staple").unwrap();
        let recovered = unwrap_key(&blob, "correct horse battery staple").unwrap();
        assert_eq!(recovered, key);
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let key = a_key();
        let blob = wrap_key(&key, "right-pass").unwrap();
        let err = unwrap_key(&blob, "wrong-pass").unwrap_err();
        assert!(
            err.contains("Wrong recovery passphrase"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn each_wrap_uses_a_fresh_salt() {
        let key = a_key();
        let a = wrap_key(&key, "p").unwrap();
        let b = wrap_key(&key, "p").unwrap();
        assert_ne!(a.salt, b.salt, "salt must be random per wrap");
        assert_ne!(a.wrapped, b.wrapped, "ciphertext must differ (salt+nonce)");
        // Both still recover the same key.
        assert_eq!(unwrap_key(&a, "p").unwrap(), key);
        assert_eq!(unwrap_key(&b, "p").unwrap(), key);
    }

    #[test]
    fn empty_passphrase_is_refused_on_wrap() {
        assert!(wrap_key(&a_key(), "").is_err());
    }

    #[test]
    fn wrap_refuses_a_non_key_input() {
        assert!(wrap_key("not-a-64-hex-key", "p").is_err());
    }

    #[test]
    fn recovery_code_roundtrips_through_string() {
        let key = a_key();
        let blob = wrap_key(&key, "pp").unwrap();
        let code = to_code(&blob);
        assert!(code.starts_with("KRECOV1."));
        let parsed = from_code(&code).unwrap();
        assert_eq!(parsed, blob);
        // …and the parsed blob still unwraps to the key.
        assert_eq!(unwrap_key(&parsed, "pp").unwrap(), key);
    }

    #[test]
    fn from_code_rejects_malformed_input() {
        assert!(from_code("garbage").is_err());
        assert!(from_code("KRECOV1.onlytwo").is_err());
        assert!(from_code("WRONGVER.YWJj.abc").is_err());
        assert!(from_code("KRECOV1.!!!notb64!!!.abc").is_err());
        assert!(from_code("KRECOV1.YWJj.").is_err()); // empty payload
    }

    #[test]
    fn tampered_wrapped_payload_is_rejected() {
        let key = a_key();
        let mut blob = wrap_key(&key, "pp").unwrap();
        // Flip a char in the wrapped base64 → AES-GCM tag must reject.
        let mut bytes = blob.wrapped.clone().into_bytes();
        let last = bytes.len() - 1;
        bytes[last] = if bytes[last] == b'A' { b'B' } else { b'A' };
        blob.wrapped = String::from_utf8(bytes).unwrap();
        assert!(unwrap_key(&blob, "pp").is_err());
    }

    #[test]
    fn the_fingerprint_tells_which_key_a_blob_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let key = a_key();
        assert_eq!(matches_key(dir.path(), &key), RecoveryMatch::Absent);
        let blob = wrap_key(&key, "pp").unwrap();
        save_blob(dir.path(), &blob).unwrap();
        assert_eq!(matches_key(dir.path(), &key), RecoveryMatch::Matches);
        assert_eq!(matches_key(dir.path(), &a_key()), RecoveryMatch::OtherKey);
        // Round trip keeps the fingerprint; a pre-0.14.3 code has none.
        assert_eq!(from_code(&to_code(&blob)).unwrap(), blob);
        let old = RecoveryBlob {
            fingerprint: None,
            ..blob
        };
        assert_eq!(to_code(&old).split('.').count(), 3);
        save_blob(dir.path(), &old).unwrap();
        assert_eq!(matches_key(dir.path(), &key), RecoveryMatch::Unverified);
        std::fs::write(dir.path().join(RECOVERY_FILENAME), "garbage").unwrap();
        assert_eq!(matches_key(dir.path(), &key), RecoveryMatch::Unreadable);
        assert!(from_code("KRECOV1.AAAAAAAAAAAAAAAAAAAAAA==.x.nothex").is_err());
    }

    #[test]
    fn wrong_salt_is_rejected() {
        // A blob whose salt was swapped derives a different KEK → unwrap fails.
        let key = a_key();
        let good = wrap_key(&key, "pp").unwrap();
        let other = wrap_key(&key, "pp").unwrap();
        let frankenstein = RecoveryBlob {
            salt: other.salt,
            wrapped: good.wrapped,
            fingerprint: None,
        };
        assert!(unwrap_key(&frankenstein, "pp").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn save_blob_never_follows_a_symlink_planted_at_the_temp_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "untouched").unwrap();
        std::os::unix::fs::symlink(
            &victim,
            dir.path().join(format!(".{}.tmp", RECOVERY_FILENAME)),
        )
        .unwrap();

        let blob = wrap_key(&a_key(), "pp").unwrap();
        save_blob(dir.path(), &blob).unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
        let saved = dir.path().join(RECOVERY_FILENAME);
        assert_eq!(load_blob(dir.path()).unwrap(), blob);
        let mode = std::fs::metadata(saved).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_then_load_blob_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_configured(dir.path()));
        assert!(load_blob(dir.path()).is_none());

        let key = a_key();
        let blob = wrap_key(&key, "pp").unwrap();
        save_blob(dir.path(), &blob).unwrap();

        assert!(is_configured(dir.path()));
        let loaded = load_blob(dir.path()).unwrap();
        assert_eq!(loaded, blob);
        assert_eq!(unwrap_key(&loaded, "pp").unwrap(), key);
    }

    #[test]
    #[cfg(unix)]
    fn saved_recovery_sidecar_is_0600_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        save_blob(dir.path(), &wrap_key(&a_key(), "pp").unwrap()).unwrap();
        let mode = std::fs::metadata(dir.path().join(RECOVERY_FILENAME))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let has_tmp = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!has_tmp);
    }
}
