//! KT-1492 — RFC 6238 time-based one-time passwords (HMAC-SHA1, 6 digits,
//! 30 s steps), the factor that cannot be driven through the UI: any
//! authenticator app (Proton, Microsoft, Google, KeePassXC) computes the same
//! code from the same seed. Pure: no clock, no storage, no state.

use sha1::{Digest, Sha1};

/// Seconds of one time step.
pub const STEP_SECS: u64 = 30;
/// Digits of a code.
pub const DIGITS: u32 = 6;
/// Steps accepted on each side of the current one (clock drift).
pub const SKEW_STEPS: u64 = 1;
/// Bytes of a seed (160 bits, RFC 4226's recommendation).
pub const SEED_BYTES: usize = 20;

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 2104 HMAC over SHA-1.
fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..20].copy_from_slice(&Sha1::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK];
    let mut outer_pad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        inner_pad[index] ^= padded[index];
        outer_pad[index] ^= padded[index];
    }
    let mut inner = Sha1::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha1::new();
    outer.update(outer_pad);
    outer.update(inner_hash);
    outer.finalize().into()
}

/// RFC 4226 dynamic truncation of the HMAC of `counter`, to `digits` digits.
fn hotp(seed: &[u8], counter: u64, digits: u32) -> u32 {
    let mac = hmac_sha1(seed, &counter.to_be_bytes());
    let offset = (mac[19] & 0x0f) as usize;
    let truncated = u32::from_be_bytes([
        mac[offset] & 0x7f,
        mac[offset + 1],
        mac[offset + 2],
        mac[offset + 3],
    ]);
    truncated % 10u32.pow(digits)
}

/// The time step `unix_secs` falls in.
pub fn step_of(unix_secs: u64) -> u64 {
    unix_secs / STEP_SECS
}

/// The code of `seed` at `step`, zero-padded.
pub fn code_at(seed: &[u8], step: u64) -> String {
    format!(
        "{:0width$}",
        hotp(seed, step, DIGITS),
        width = DIGITS as usize
    )
}

/// Equality that does not stop at the first differing byte.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The step `code` is valid for around `unix_secs` (±[`SKEW_STEPS`]), if any.
/// The caller refuses a step it has already accepted: a used code is refused
/// on replay.
pub fn verify(seed: &[u8], code: &str, unix_secs: u64) -> Option<u64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS as usize || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let now = step_of(unix_secs);
    let mut found = None;
    // Every candidate is compared: the time taken does not say which matched.
    for step in now.saturating_sub(SKEW_STEPS)..=now + SKEW_STEPS {
        if same(code_at(seed, step).as_bytes(), code.as_bytes()) {
            found = Some(step);
        }
    }
    found
}

/// RFC 4648 base32 without padding, as authenticator apps take a manual key.
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            out.push(BASE32[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(BASE32[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// The bytes of a base32 text (spaces, dashes and case ignored).
pub fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        if c == ' ' || c == '-' || c == '=' {
            continue;
        }
        let value = BASE32
            .iter()
            .position(|b| *b as char == c.to_ascii_uppercase())? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            out.push((buffer >> (bits - 8)) as u8);
            bits -= 8;
        }
    }
    Some(out)
}

/// The `otpauth://` URI an authenticator app scans (the QR code carries it).
pub fn provisioning_uri(issuer: &str, account: &str, seed: &[u8]) -> String {
    fn encode(text: &str) -> String {
        text.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                other => format!("%{other:02X}"),
            })
            .collect()
    }
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECS}",
        encode(issuer),
        encode(account),
        base32_encode(seed),
        encode(issuer),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RFC_SEED: &[u8] = b"12345678901234567890";

    /// RFC 6238 appendix B, SHA-1, last 6 of the 8-digit reference values.
    #[test]
    fn matches_the_rfc_6238_reference_vectors() {
        for (time, expected) in [
            (59u64, "287082"),
            (1_111_111_109, "081804"),
            (1_111_111_111, "050471"),
            (1_234_567_890, "005924"),
            (2_000_000_000, "279037"),
        ] {
            assert_eq!(code_at(RFC_SEED, step_of(time)), expected, "at {time}");
        }
    }

    #[test]
    fn hmac_sha1_matches_rfc_2202() {
        let mac = hmac_sha1(&[0x0b; 20], b"Hi There");
        let hex: String = mac.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "b617318655057264e28bc0b6fb378c8ef146be00");
    }

    #[test]
    fn accepts_one_step_each_side_and_no_more() {
        let now = 1_700_000_000;
        let step = step_of(now);
        for offset in [step - 1, step, step + 1] {
            assert_eq!(
                verify(RFC_SEED, &code_at(RFC_SEED, offset), now),
                Some(offset)
            );
        }
        for offset in [step - 2, step + 2] {
            assert_eq!(verify(RFC_SEED, &code_at(RFC_SEED, offset), now), None);
        }
    }

    #[test]
    fn refuses_what_is_not_six_digits() {
        let now = 1_700_000_000;
        for bad in ["", "12345", "1234567", "abcdef", "12 34", "１２３４５６"] {
            assert_eq!(verify(RFC_SEED, bad, now), None, "{bad:?}");
        }
        let good = code_at(RFC_SEED, step_of(now));
        assert!(verify(RFC_SEED, &format!(" {} ", good), now).is_some());
    }

    #[test]
    fn base32_round_trips_and_matches_the_rfc_4648_vectors() {
        assert_eq!(base32_encode(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            base32_decode("mzxw 6ytb-oi").as_deref(),
            Some(&b"foobar"[..])
        );
        assert_eq!(base32_decode("1"), None);
        let seed: Vec<u8> = (0..SEED_BYTES as u8).collect();
        assert_eq!(base32_decode(&base32_encode(&seed)), Some(seed));
    }

    #[test]
    fn the_provisioning_uri_carries_the_seed_and_the_parameters() {
        let uri = provisioning_uri("Kronn", "me@host", RFC_SEED);
        assert!(uri
            .starts_with("otpauth://totp/Kronn:me%40host?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"));
        assert!(
            uri.contains("digits=6") && uri.contains("period=30") && uri.contains("issuer=Kronn")
        );
    }
}
