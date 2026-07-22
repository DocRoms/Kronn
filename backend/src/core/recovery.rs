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
    // The unwrapped value must itself be a valid key (defense in depth).
    crypto::parse_secret(&key_hex)?;
    Ok(key_hex)
}

/// Serialize a blob to a portable "recovery code" string the user can save
/// off-machine. Safe to expose — useless without the passphrase.
pub fn to_code(blob: &RecoveryBlob) -> String {
    // `wrapped` is standard base64 (no '.'), so '.' is an unambiguous delimiter.
    format!("{}.{}.{}", CODE_PREFIX, B64.encode(blob.salt), blob.wrapped)
}

/// Parse a recovery code back into a blob.
pub fn from_code(code: &str) -> Result<RecoveryBlob, String> {
    let parts: Vec<&str> = code.trim().split('.').collect();
    if parts.len() != 3 || parts[0] != CODE_PREFIX {
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
    Ok(RecoveryBlob {
        salt,
        wrapped: parts[2].to_string(),
    })
}

/// Persist the recovery code to the `0600` sidecar in `dir` (atomic temp+rename
/// in the same dir, mirroring `keyvault::SidecarFile`).
pub fn save_blob(dir: &Path, blob: &RecoveryBlob) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(RECOVERY_FILENAME);
    let tmp = dir.join(format!(".{}.tmp", RECOVERY_FILENAME));
    std::fs::write(&tmp, to_code(blob).as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)
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
    fn wrong_salt_is_rejected() {
        // A blob whose salt was swapped derives a different KEK → unwrap fails.
        let key = a_key();
        let good = wrap_key(&key, "pp").unwrap();
        let other = wrap_key(&key, "pp").unwrap();
        let frankenstein = RecoveryBlob {
            salt: other.salt,
            wrapped: good.wrapped,
        };
        assert!(unwrap_key(&frankenstein, "pp").is_err());
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
