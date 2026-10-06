//! Scrubbing by value: every credential a call resolved, in every wire form
//! it may come back in, is replaced before any text leaves the call (step
//! output, summaries, errors, logs).
//!
//! Name-based redaction (`core::redact`) cannot see a secret placed in a
//! harmless slot (`?q=${ENV.API_KEY}`) or echoed bare by a server; this can.
//! It cannot help with text stored before it existed: history only gets the
//! heuristics.

use base64::Engine as _;

/// Secrets shorter than this are replaced only as whole tokens, so a short
/// PIN does not mangle every number or word that contains it.
pub const MIN_ANYWHERE_LEN: usize = 8;

const MASK: &str = "***";

#[derive(Debug, Clone, Default)]
pub struct SecretSet {
    /// Every form to replace, longest first at scrub time.
    forms: Vec<String>,
}

fn percent_encode(raw: &str, space_as_plus: bool, lower_hex: bool) -> String {
    let mut out = String::with_capacity(raw.len() * 3);
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            b' ' if space_as_plus => out.push('+'),
            _ if lower_hex => out.push_str(&format!("%{byte:02x}")),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl SecretSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    /// Adds `secret` in its raw, URL-encoded (path, form, both hex cases),
    /// base64 (standard and URL-safe, padded or not) and JSON-escaped forms.
    pub fn add(&mut self, secret: &str) {
        let secret = secret.trim();
        if secret.is_empty() {
            return;
        }
        let engines = [
            base64::engine::general_purpose::STANDARD.encode(secret),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret),
            base64::engine::general_purpose::URL_SAFE.encode(secret),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
        ];
        let json = serde_json::to_string(secret).unwrap_or_default();
        let json_inner = json
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or("")
            .to_string();
        let mut forms = vec![
            secret.to_string(),
            percent_encode(secret, false, false),
            percent_encode(secret, false, true),
            percent_encode(secret, true, false),
            percent_encode(secret, true, true),
            json_inner,
        ];
        // A base64 form of a very short secret is itself short and generic.
        if secret.len() >= 4 {
            forms.extend(engines);
        }
        for form in forms {
            if !form.is_empty() && !self.forms.contains(&form) {
                self.forms.push(form);
            }
        }
    }

    /// HTTP Basic: the password, and the base64 `user:pass` the header holds.
    pub fn add_basic(&mut self, user: &str, password: &str) {
        self.add(password);
        self.add(&format!("{user}:{password}"));
    }

    pub fn extend(&mut self, other: &SecretSet) {
        for form in &other.forms {
            if !self.forms.contains(form) {
                self.forms.push(form.clone());
            }
        }
    }

    pub fn scrub(&self, text: &str) -> String {
        if self.forms.is_empty() || text.is_empty() {
            return text.to_string();
        }
        let mut forms: Vec<&String> = self.forms.iter().collect();
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        let mut out = text.to_string();
        for form in forms {
            if form.len() >= MIN_ANYWHERE_LEN {
                out = out.replace(form.as_str(), MASK);
            } else {
                out = replace_whole_tokens(&out, form);
            }
        }
        out
    }
}

/// Replaces `needle` only where it is not glued to other alphanumerics.
fn replace_whole_tokens(text: &str, needle: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(needle) {
        let before = rest[..index].chars().next_back();
        let after = rest[index + needle.len()..].chars().next();
        let bounded = !before.is_some_and(is_word) && !after.is_some_and(is_word);
        out.push_str(&rest[..index]);
        out.push_str(if bounded { MASK } else { needle });
        rest = &rest[index + needle.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_form_of_a_secret_is_masked() {
        let mut set = SecretSet::new();
        set.add("s3cr3t/value+key=");
        let text = format!(
            "raw s3cr3t/value+key= url {} form {} b64 {} json {}",
            "s3cr3t%2Fvalue%2Bkey%3D",
            "s3cr3t%2fvalue%2bkey%3d",
            base64::engine::general_purpose::STANDARD.encode("s3cr3t/value+key="),
            serde_json::to_string("s3cr3t/value+key=").unwrap(),
        );
        let out = set.scrub(&text);
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(!out.contains("czNjcjN0"), "{out}");
    }

    #[test]
    fn basic_auth_header_value_and_password_are_masked() {
        let mut set = SecretSet::new();
        set.add_basic("user@example.com", "ATATT-pass");
        let header =
            base64::engine::general_purpose::STANDARD.encode("user@example.com:ATATT-pass");
        let out = set.scrub(&format!("Basic {header} / ATATT-pass"));
        assert!(!out.contains(&header), "{out}");
        assert!(!out.contains("ATATT-pass"), "{out}");
    }

    #[test]
    fn a_short_secret_is_masked_only_as_a_whole_token() {
        let mut set = SecretSet::new();
        set.add("4821");
        assert_eq!(set.scrub("pin=4821, id 148210"), "pin=***, id 148210");
    }
}
