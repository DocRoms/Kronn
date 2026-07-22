use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};

const NONCE_LEN: usize = 12;

/// Encrypt a plaintext string using AES-256-GCM.
/// Returns a base64-encoded string: nonce || ciphertext.
pub fn encrypt(plaintext: &str, key: &[u8; 32]) -> Result<String, String> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(B64.encode(combined))
}

/// Decrypt a base64-encoded string (nonce || ciphertext) back to plaintext.
pub fn decrypt(encoded: &str, key: &[u8; 32]) -> Result<String, String> {
    let combined = B64
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    if combined.len() < NONCE_LEN + 1 {
        return Err("Ciphertext too short".into());
    }

    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key.into());

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {}", e))?;

    String::from_utf8(plaintext).map_err(|e| format!("UTF-8 decode failed: {}", e))
}

/// Generate a random 32-byte key, returned as hex string (64 chars).
pub fn generate_secret() -> String {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    hex::encode(&key)
}

/// Parse a hex-encoded secret string into a 32-byte key.
pub fn parse_secret(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex secret: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!("Secret must be 32 bytes, got {}", bytes.len()));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Mask a string for display: show first 2 and last 2 chars.
pub fn mask_value(value: &str) -> String {
    if value.len() <= 6 {
        return "*".repeat(value.len());
    }
    format!("{}...{}", &value[..2], &value[value.len() - 2..])
}

/// Domain-separated fingerprint ("KID") of a 32-byte encryption key.
///
/// Lets us record *which* key encrypted a row and later check a candidate key
/// against it WITHOUT storing or exposing the key itself. Returns 16 lowercase
/// hex chars (first 8 bytes of SHA-256 over a fixed domain tag + the key).
/// One-way and collision-resistant for our purpose — telling two 256-bit keys
/// apart — and the truncation keeps it compact for a DB column.
pub fn key_fingerprint(key: &[u8; 32]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"kronn-kid-v1");
    hasher.update(key);
    let result = hasher.finalize();
    // sha2 0.11 returns `hybrid_array::Array` (no `LowerHex`) — manual hex, as
    // in `core::checksums`. First 8 bytes are ample to distinguish keys.
    result
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// KID from a hex-encoded secret string — convenience for the config / keystore
/// call sites that hold the hex form. Errors if the hex isn't a valid 32-byte key.
pub fn key_fingerprint_hex(hex_secret: &str) -> Result<String, String> {
    let key = parse_secret(hex_secret)?;
    Ok(key_fingerprint(&key))
}

// hex encode/decode (tiny, no extra dep needed)
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        if !s.len().is_multiple_of(2) {
            return Err("Odd-length hex string".into());
        }
        (0..s.len())
            .step_by(2)
            .map(|i| {
                u8::from_str_radix(&s[i..i + 2], 16)
                    .map_err(|e| format!("Invalid hex at {}: {}", i, e))
            })
            .collect()
    }
}

// 0.8.7 — P0-1 of the QA roadmap. The whole MCP env-secret pipeline rests
// on this 91-LOC module (Aes256Gcm + base64 framing). Zero tests until now
// would mean any silent regression here loses every user's saved API keys
// without a peep. The suite below pins the contract end-to-end :
// roundtrip, AEAD tamper detection, wrong-key rejection, malformed input
// rejection, generate_secret uniqueness + parse_secret validation.

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key(seed: u8) -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        k
    }

    #[test]
    fn encrypt_decrypt_roundtrip_ascii() {
        let key = make_key(0xA1);
        let pt = "GITHUB_PERSONAL_ACCESS_TOKEN=ghp_abcdefghijklmnop";
        let ct = encrypt(pt, &key).expect("encrypt OK");
        // Ciphertext never leaks the plaintext (Base64 of nonce||ciphertext;
        // the pt would never be embedded verbatim under a real AEAD).
        assert!(!ct.contains("ghp_"));
        let back = decrypt(&ct, &key).expect("decrypt OK");
        assert_eq!(back, pt);
    }

    #[test]
    fn encrypt_decrypt_roundtrip_empty() {
        let key = make_key(0);
        let ct = encrypt("", &key).expect("empty plaintext is valid");
        let back = decrypt(&ct, &key).expect("decrypt OK");
        assert_eq!(back, "");
    }

    #[test]
    fn encrypt_decrypt_roundtrip_utf8_and_emoji() {
        // Real ENV values can carry accented chars (FR labels in custom
        // plugins) and even emoji. UTF-8 must survive the bytes round-trip.
        let key = make_key(0x5A);
        let pt = "clé:été✨ — naïve / accentué — 漢字 — \n\t\0";
        let ct = encrypt(pt, &key).expect("encrypt OK");
        let back = decrypt(&ct, &key).expect("decrypt OK");
        assert_eq!(back, pt);
    }

    #[test]
    fn encrypt_decrypt_roundtrip_large() {
        // Some custom plugins ship JSON schemas in env values (~4-8 KB).
        let key = make_key(0x33);
        let pt: String = (0..8_192).map(|i| ((i % 95) as u8 + 32) as char).collect();
        let ct = encrypt(&pt, &key).expect("encrypt OK");
        let back = decrypt(&ct, &key).expect("decrypt OK");
        assert_eq!(back, pt);
    }

    #[test]
    fn encrypt_produces_different_ciphertext_for_same_plaintext() {
        // Nonce is randomised on every call — two encrypts of the same
        // plaintext under the same key MUST diverge (otherwise a leaked
        // ciphertext could be matched against a known pt).
        let key = make_key(0x77);
        let a = encrypt("same-plaintext", &key).unwrap();
        let b = encrypt("same-plaintext", &key).unwrap();
        assert_ne!(a, b, "nonce reuse would be a hard-security regression");
    }

    #[test]
    fn decrypt_rejects_aead_tag_tampering() {
        // Flip one byte of the ciphertext body — AEAD must refuse, not
        // return garbled plaintext. This is the single most important
        // anti-tampering guarantee of AES-GCM.
        let key = make_key(0x42);
        let ct = encrypt("secret-payload", &key).unwrap();
        let mut bytes = B64.decode(&ct).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = B64.encode(&bytes);
        let err = decrypt(&tampered, &key).expect_err("tampering must fail");
        assert!(err.contains("Decryption failed"), "unexpected error: {err}");
    }

    #[test]
    fn decrypt_rejects_wrong_key() {
        let k1 = make_key(0x10);
        let k2 = make_key(0x20);
        let ct = encrypt("payload", &k1).unwrap();
        let err = decrypt(&ct, &k2).expect_err("wrong key must fail");
        assert!(err.contains("Decryption failed"), "unexpected error: {err}");
    }

    #[test]
    fn decrypt_rejects_truncated_ciphertext() {
        // A ciphertext shorter than nonce + 1 byte cannot possibly be
        // valid — caller-friendly explicit error instead of an aead panic.
        let key = make_key(0xFF);
        let err = decrypt(&B64.encode([1u8; 5]), &key).expect_err("too short");
        assert!(err.contains("too short"), "unexpected error: {err}");
    }

    #[test]
    fn decrypt_rejects_invalid_base64() {
        let key = make_key(0x01);
        let err = decrypt("===this-is-not-base64===", &key).expect_err("bad b64");
        assert!(err.contains("Base64"), "unexpected error: {err}");
    }

    #[test]
    fn generate_secret_is_64_hex_chars_and_unique_per_call() {
        let s1 = generate_secret();
        let s2 = generate_secret();
        assert_eq!(s1.len(), 64);
        assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));
        // Collision in 256-bit space is astronomically unlikely. Two calls
        // returning the same value flags an OsRng wiring bug.
        assert_ne!(
            s1, s2,
            "generate_secret must produce fresh entropy each call"
        );
    }

    #[test]
    fn parse_secret_roundtrip_with_generate_secret() {
        let hex = generate_secret();
        let key = parse_secret(&hex).expect("valid hex must parse");
        assert_eq!(key.len(), 32);
        // The parsed key must actually decrypt what its hex twin encrypted.
        let ct = encrypt("token-value", &key).unwrap();
        let pt = decrypt(&ct, &key).unwrap();
        assert_eq!(pt, "token-value");
    }

    #[test]
    fn parse_secret_rejects_wrong_length() {
        let short = "deadbeef".to_string();
        let err = parse_secret(&short).expect_err("wrong length must fail");
        assert!(err.contains("32 bytes"), "unexpected error: {err}");
    }

    #[test]
    fn parse_secret_rejects_non_hex() {
        let err = parse_secret("zz".repeat(32).as_str()).expect_err("non-hex must fail");
        assert!(err.contains("Invalid hex"), "unexpected error: {err}");
    }

    #[test]
    fn parse_secret_rejects_odd_length() {
        let err = parse_secret("abc").expect_err("odd-length must fail");
        assert!(err.contains("Odd-length"), "unexpected error: {err}");
    }

    #[test]
    fn mask_value_short_strings_fully_masked() {
        // Display safety : values ≤ 6 chars are entirely masked (otherwise
        // the prefix/suffix tail would dominate and leak the secret).
        assert_eq!(mask_value(""), "");
        assert_eq!(mask_value("a"), "*");
        assert_eq!(mask_value("abcdef"), "******");
    }

    #[test]
    fn mask_value_longer_strings_show_first_two_and_last_two() {
        assert_eq!(mask_value("abcdefg"), "ab...fg");
        assert_eq!(mask_value("ghp_abcdefghijklmnop"), "gh...op");
    }

    #[test]
    fn key_fingerprint_is_stable_and_16_hex() {
        let key = parse_secret(&generate_secret()).unwrap();
        let kid = key_fingerprint(&key);
        assert_eq!(kid.len(), 16, "KID is 16 hex chars");
        assert!(kid.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic: the same key must yield the same KID across calls.
        assert_eq!(kid, key_fingerprint(&key));
    }

    #[test]
    fn key_fingerprint_differs_for_different_keys() {
        let a = parse_secret(&generate_secret()).unwrap();
        let b = parse_secret(&generate_secret()).unwrap();
        assert_ne!(key_fingerprint(&a), key_fingerprint(&b));
    }

    #[test]
    fn key_fingerprint_is_not_the_key_itself() {
        // A leaked KID must not be the key: different length (16 vs 64 hex).
        let hex = generate_secret();
        let key = parse_secret(&hex).unwrap();
        let kid = key_fingerprint(&key);
        assert_ne!(kid, hex);
        assert_ne!(kid.len(), hex.len());
    }

    #[test]
    fn key_fingerprint_hex_matches_byte_form_and_rejects_bad_hex() {
        let hex = generate_secret();
        let key = parse_secret(&hex).unwrap();
        assert_eq!(key_fingerprint_hex(&hex).unwrap(), key_fingerprint(&key));
        assert!(key_fingerprint_hex("nothex").is_err());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // No matter the plaintext, encrypt→decrypt is lossless under the same key.
        #[test]
        fn roundtrip_any_plaintext(pt in ".{0,4096}", seed in any::<[u8; 32]>()) {
            let key = seed; // any 32 bytes is a valid AES-256 key
            let ct = encrypt(&pt, &key).unwrap();
            prop_assert_eq!(decrypt(&ct, &key).unwrap(), pt);
        }

        // AEAD integrity: flipping ANY single byte of the base64-decoded
        // (nonce||ct||tag) blob must make decryption FAIL — never yield a
        // different-but-valid plaintext. Generalises the two hand-written tamper
        // tests into "no bit-flip anywhere ever decrypts".
        #[test]
        fn any_single_byte_tamper_is_rejected(
            pt in ".{1,256}", seed in any::<[u8; 32]>(), idx in any::<usize>(), xor in 1u8..=255,
        ) {
            let key = seed;
            let ct_b64 = encrypt(&pt, &key).unwrap();
            let mut raw = B64.decode(&ct_b64).unwrap();
            let i = idx % raw.len();
            raw[i] ^= xor; // guaranteed-different byte
            let tampered = B64.encode(&raw);
            prop_assert!(decrypt(&tampered, &key).is_err());
        }

        // parse_secret accepts exactly well-formed 64-hex strings and the parsed
        // key roundtrips; any other length is rejected, never panics.
        #[test]
        fn parse_secret_accepts_only_valid_64_hex(bytes in any::<[u8; 32]>()) {
            let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
            let key = parse_secret(&hex).unwrap();
            prop_assert_eq!(&key, &bytes);
            let ct = encrypt("x", &key).unwrap();
            prop_assert_eq!(decrypt(&ct, &key).unwrap(), "x".to_string());
        }

        #[test]
        fn parse_secret_rejects_wrong_length_hex(len in (0usize..128).prop_filter("not 64", |n| *n != 64)) {
            let hex = "a".repeat(len);
            prop_assert!(parse_secret(&hex).is_err());
        }
    }
}
