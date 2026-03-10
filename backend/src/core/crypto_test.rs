#[cfg(test)]
mod tests {
    use crate::core::crypto::*;

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let key = parse_secret(&generate_secret()).unwrap();
        let plaintext = "hello world, this is a secret!";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypt_decrypt_empty_string() {
        let key = parse_secret(&generate_secret()).unwrap();
        let encrypted = encrypt("", &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, "");
    }

    #[test]
    fn encrypt_decrypt_unicode() {
        let key = parse_secret(&generate_secret()).unwrap();
        let plaintext = "Héllo wörld 你好 🔐";
        let encrypted = encrypt(plaintext, &key).unwrap();
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn decrypt_wrong_key_fails() {
        let key1 = parse_secret(&generate_secret()).unwrap();
        let key2 = parse_secret(&generate_secret()).unwrap();
        let encrypted = encrypt("secret", &key1).unwrap();
        assert!(decrypt(&encrypted, &key2).is_err());
    }

    #[test]
    fn decrypt_invalid_base64_fails() {
        let key = parse_secret(&generate_secret()).unwrap();
        assert!(decrypt("not-valid-base64!!!", &key).is_err());
    }

    #[test]
    fn decrypt_too_short_fails() {
        let key = parse_secret(&generate_secret()).unwrap();
        // Base64 of just a few bytes — too short for nonce + ciphertext
        assert!(decrypt("AQID", &key).is_err());
    }

    #[test]
    fn generate_secret_length() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 64); // 32 bytes = 64 hex chars
    }

    #[test]
    fn generate_secret_uniqueness() {
        let s1 = generate_secret();
        let s2 = generate_secret();
        assert_ne!(s1, s2);
    }

    #[test]
    fn parse_secret_valid() {
        let hex = "a".repeat(64);
        let key = parse_secret(&hex).unwrap();
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn parse_secret_wrong_length() {
        assert!(parse_secret("aabb").is_err());
    }

    #[test]
    fn parse_secret_invalid_hex() {
        let bad = "zz".repeat(32);
        assert!(parse_secret(&bad).is_err());
    }

    #[test]
    fn parse_secret_odd_length() {
        assert!(parse_secret("abc").is_err());
    }

    #[test]
    fn mask_value_long() {
        assert_eq!(mask_value("sk-abc123xyz"), "sk...yz");
    }

    #[test]
    fn mask_value_short() {
        assert_eq!(mask_value("abc"), "***");
    }

    #[test]
    fn mask_value_exact_boundary() {
        // 6 chars = boundary, should be fully masked
        assert_eq!(mask_value("abcdef"), "******");
        // 7 chars = first 2 + last 2
        assert_eq!(mask_value("abcdefg"), "ab...fg");
    }

    #[test]
    fn mask_value_empty() {
        assert_eq!(mask_value(""), "");
    }

    #[test]
    fn each_encryption_produces_different_ciphertext() {
        let key = parse_secret(&generate_secret()).unwrap();
        let e1 = encrypt("same text", &key).unwrap();
        let e2 = encrypt("same text", &key).unwrap();
        // Different nonces mean different ciphertexts
        assert_ne!(e1, e2);
        // But both decrypt to the same plaintext
        assert_eq!(decrypt(&e1, &key).unwrap(), "same text");
        assert_eq!(decrypt(&e2, &key).unwrap(), "same text");
    }
}
