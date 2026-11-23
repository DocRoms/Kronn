//! Scrubbing by value: every credential a call resolved, in every wire form
//! it may come back in, is replaced before any text leaves the call (step
//! output, summaries, errors, logs).
//!
//! Name-based redaction (`core::redact`) cannot see a secret placed in a
//! harmless slot (`?q=${ENV.API_KEY}`) or echoed bare by a server; this can.
//! It cannot help with text stored before it existed: history only gets the
//! heuristics. Known limits: an endpoint that transforms what it echoes
//! (another encoding, a split, a cipher) defeats any finite list of forms,
//! and a secret under [`MIN_ANYWHERE_LEN`] glued inside another token is not
//! masked.

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

/// What the URL serializer sends for `secret` written raw into a URL,
/// derived from `url` (through `reqwest::Url`) rather than from a list of
/// characters, so a `url` release that escapes another character is followed
/// without a code change:
///
/// - in a path (`https://h/bot{TOKEN}/x` parsed by `Url::parse`): only the
///   path percent-encode set is escaped (`^` -> `%5E` since url 2.5.9, `:`
///   kept). A `?` or `#` in the value ends the path there, the rest is
///   serialised with the query or fragment set: the form is the whole tail.
///   A `/` stays a separator (raw), a `\` becomes `/` (special schemes). The
///   value is probed alone (`http://h/{secret}`) and glued between two
///   letters (`http://h/a{secret}z`), because dot segments are resolved by
///   what surrounds them: `x/..` alone collapses to nothing, glued it is sent
///   as `x/..z`;
/// - in a raw query (`/x?key={KEY}`): the query percent-encode set (`'`
///   escaped, `^` kept), a `#` starting the fragment;
/// - as a query value added by `query_pairs_mut` (form encoding, space as
///   `+`, `*` kept raw);
/// - after a query round trip (`query_pairs()` read back and written again
///   with `query_pairs_mut()`, as `strip_query_keys` does on a cross-origin
///   redirect and link-header pagination does for the next page): written
///   raw, decoded as a form (`+` read as a space) and form-encoded again,
///   `AbC+dEf/ghIJ==` comes back `AbC+dEf%2FghIJ%3D%3D`. A `&` or `=` in
///   the value splits it into pairs: the form is the whole tail;
/// - in a fragment (`/x#{secret}`): the fragment set;
/// - in user info: as the password (`set_password`, `@` and `:` escaped), as
///   the user name (`set_username`), and as the whole user info
///   (`http://{secret}@h/`, its first `:` the separator);
/// - as a host label (`https://{TENANT}.example.com`): see
///   [`host_label_form`], the IDNA form of a non-ASCII value.
///
/// Percent-escaping is per character, so a form of the path, query, fragment
/// and user-info probes is a substring of the URL that holds the value in
/// that slot. It is not when the URL resolves the value's dot segments
/// differently from both probes (a value starting with `../` glued after a
/// segment that is itself a dot segment), nor for a host label: IDNA's
/// Punycode encodes the whole label, so a non-ASCII value glued inside one
/// (`https://api-{TENANT}.example.com`) is sent in a form the value alone
/// does not give; only a value that is a whole label is covered.
///
/// A tab or a newline inside the value is removed by the parser, so every
/// form above follows it; the one limit left is a form that falls under
/// [`MIN_ANYWHERE_LEN`] bytes once they are gone: it is then masked only as
/// a whole token, like any short secret. The floor is kept on purpose: it
/// keeps short generic text out of the masks applied anywhere in a word.
fn url_serialized_forms(secret: &str) -> Vec<String> {
    const BASE: &str = "http://h/";
    let mut forms = Vec::new();
    let between = |text: &str, prefix: &str, suffix: &str| {
        text.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .map(str::to_string)
    };
    let parsed = |text: String| reqwest::Url::parse(&text).ok();
    let probes = [
        (format!("{BASE}{secret}"), BASE, ""),
        (format!("{BASE}a{secret}z"), "http://h/a", "z"),
        (format!("{BASE}?{secret}"), "http://h/?", ""),
        (format!("{BASE}#{secret}"), "http://h/#", ""),
        (format!("http://{secret}@h/"), "http://", "@h/"),
    ];
    for (text, prefix, suffix) in probes {
        if let Some(url) = parsed(text) {
            forms.extend(between(url.as_str(), prefix, suffix));
        }
    }
    if let Some(mut url) = parsed(BASE.to_string()) {
        url.query_pairs_mut().append_pair("k", secret);
        forms.extend(between(url.as_str(), "http://h/?k=", ""));
    }
    if let Some(mut url) = parsed(format!("{BASE}?k={secret}")) {
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        url.query_pairs_mut().clear().extend_pairs(pairs);
        forms.extend(between(url.as_str(), "http://h/?k=", ""));
    }
    if let Some(mut url) = parsed(BASE.to_string()) {
        if url.set_password(Some(secret)).is_ok() {
            forms.extend(url.password().map(str::to_string));
        }
    }
    if let Some(mut url) = parsed(BASE.to_string()) {
        if url.set_username(secret).is_ok() {
            forms.push(url.username().to_string());
        }
    }
    forms.extend(host_label_form(secret));
    forms
}

/// The form the `url` crate sends `secret` in as a host label
/// (`http://{secret}.h/`, the host without its `.h`): a non-ASCII value goes
/// out in its IDNA form (`xn--…`), which no other form matches. Only a value
/// that is a whole label: Punycode encodes the whole label, so a value glued
/// to other text inside one (`api-{TENANT}`) is sent in another form. `None` when
/// it is the ASCII-lowercased value itself (an ordinary ASCII value adds
/// nothing) or the value percent-decoded (`%7B%7D` is sent `{}` in a host:
/// [`SecretSet::add`] registers a decoded value from [`MIN_ANYWHERE_LEN`]
/// bytes, and a shorter one masked as a whole token would break JSON), and
/// when the value does not land whole in the host (a `/`, `?`, `#`, `@` or
/// `:` in it moves part of it to another component).
fn host_label_form(secret: &str) -> Option<String> {
    let url = reqwest::Url::parse(&format!("http://{secret}.h/")).ok()?;
    let whole = url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    let label = url.host_str()?.strip_suffix(".h")?;
    let new = label != secret.to_ascii_lowercase()
        && label != percent_decode(secret).to_ascii_lowercase();
    (whole && !label.is_empty() && new).then(|| label.to_string())
}

/// `form` with the hex digits of its `%XX` escapes in lower case.
fn lower_hex_escapes(form: &str) -> String {
    let mut bytes = form.as_bytes().to_vec();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'%' && bytes[i + 1].is_ascii_hexdigit() && bytes[i + 2].is_ascii_hexdigit()
        {
            bytes[i + 1] = bytes[i + 1].to_ascii_lowercase();
            bytes[i + 2] = bytes[i + 2].to_ascii_lowercase();
            i += 3;
        } else {
            i += 1;
        }
    }
    // Only ASCII hex digits changed: still valid UTF-8.
    String::from_utf8(bytes).unwrap_or_else(|_| form.to_string())
}

/// `%XX` escapes decoded, anything else kept, invalid UTF-8 replaced: the
/// value behind a component `url` hands back serialised (`path_segments()`,
/// `password()`, `query()`), which a server may echo decoded.
pub fn percent_decode(serialized: &str) -> String {
    let bytes = serialized.as_bytes();
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Each pair's value as written in `url.query()` (split on `&`, then on the
/// first `=`; an empty value gives nothing): `query_pairs()` hands it back
/// form-decoded (`AbC+dEf` as `AbC dEf`), while a server echoes it as
/// written. A pair with no `=` (`?SecretTokenWithoutEquals`) is a value
/// whole: `query_pairs()` hands it back as a key with an empty value.
pub fn literal_query_values(url: &reqwest::Url) -> Vec<&str> {
    url.query()
        .into_iter()
        .flat_map(|query| query.split('&'))
        .map(|pair| pair.split_once('=').map_or(pair, |(_, value)| value))
        .filter(|value| !value.is_empty())
        .collect()
}

/// The bare tokens of `url`'s query (a pair with no `=`, `?AbC+dEf/ghIJkl`)
/// form-decoded, as `query_pairs()` hands them back as keys: a server that
/// reads the query as a form echoes `AbC dEf/ghIJkl` (the `+` read as a
/// space). [`literal_query_values`] gives the token as written and its
/// percent-decoding; this gives the third reading.
///
/// `query_pairs()` skips an empty pair (`&&`) exactly as the filter below
/// does, so the two sequences stay aligned.
pub fn bare_query_tokens(url: &reqwest::Url) -> Vec<String> {
    let literal_pairs = url
        .query()
        .into_iter()
        .flat_map(|query| query.split('&'))
        .filter(|pair| !pair.is_empty());
    literal_pairs
        .zip(url.query_pairs())
        .filter(|(literal, _)| !literal.contains('='))
        .map(|(_, (key, _))| key.into_owned())
        .filter(|key| !key.is_empty())
        .collect()
}

/// Adds the `;`-separated pieces of a query value or a path segment to
/// `set`, each raw and percent-decoded: neither the WHATWG parser nor
/// [`literal_query_values`] splits on `;`, so `token=AbC+dEf/ghIJ==;v=1`
/// (or a path token followed by matrix parameters, `AbCdEf123456;v=1`) gives
/// one value glued to `;v=1`, and a server echoing the token alone was not
/// masked. The first piece is added from `first_min_len` bytes (the rule the
/// caller applies to a value), every later one from [`MIN_ANYWHERE_LEN`]
/// (`v=1` would mask that text wherever it stands). A later piece that is
/// itself `key=value` (`v=1;token=AbCdEf123456`) also adds what follows its
/// first `=`, raw and percent-decoded, from [`MIN_ANYWHERE_LEN`] bytes. A
/// value with no `;` adds nothing.
pub fn add_semicolon_pieces(set: &mut SecretSet, value: &str, first_min_len: usize) {
    if !value.contains(';') {
        return;
    }
    let mut add = |piece: &str, min_len: usize| {
        for form in [piece.to_string(), percent_decode(piece)] {
            if !form.trim().is_empty() && form.trim().len() >= min_len {
                set.add(&form);
            }
        }
    };
    for (index, piece) in value.split(';').enumerate() {
        if index == 0 {
            add(piece, first_min_len);
            continue;
        }
        add(piece, MIN_ANYWHERE_LEN);
        if let Some((_, after)) = piece.split_once('=') {
            add(after, MIN_ANYWHERE_LEN);
        }
    }
}

/// Adds the credentials an `Authorization: Basic …` value carries (scheme
/// matched ASCII case-insensitively): the base64 text, and, decoded, the
/// password and `user:password` (see [`SecretSet::add_basic`]), which a
/// server answering 401 « invalid password … » may echo. Any other value adds
/// nothing. Shared by the API call and the notify step.
pub fn add_basic_authorization(set: &mut SecretSet, value: &str) {
    let value = value.trim();
    let Some((scheme, encoded)) = value.split_once(' ') else {
        return;
    };
    if !scheme.eq_ignore_ascii_case("basic") {
        return;
    }
    let encoded = encoded.trim();
    set.add(encoded);
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok());
    if let Some((user, password)) = decoded.as_deref().and_then(|d| d.split_once(':')) {
        set.add_basic(user, password);
    }
}

impl SecretSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    /// Adds `secret` in its raw, fully URL-encoded (path, form, both hex
    /// cases), base64 (standard and URL-safe, padded or not) and
    /// JSON-escaped forms, plus the forms the `url` crate gives it when it is
    /// placed raw in a URL (see [`url_serialized_forms`]): in a path (alone
    /// and glued), in a raw query, as a form-encoded query value, after a
    /// query round trip, in a fragment, in user info and as a host label
    /// (IDNA). Those are computed by the crate itself, so a `url` release
    /// that escapes another character is followed without a code change.
    ///
    /// A form under [`MIN_ANYWHERE_LEN`] bytes is matched case-sensitively as
    /// a whole token: one that holds a percent escape is also added with
    /// lower-case hex digits.
    ///
    /// Each form is also added with its trailing slashes removed (when that
    /// leaves something): `build_url` trims them off a base URL, so a secret
    /// ending in `/` at the end of one is sent without them. A secret made
    /// almost only of slashes (`a///////`) gives a trimmed form of one byte
    /// (`a`), masked only as a whole token, like any short secret.
    ///
    /// A secret stored pre-encoded (`Abc%5Edef%3Aghi`) is echoed decoded by
    /// a server (`Abc^def:ghi`): its [`percent_decode`], when it differs and
    /// is [`MIN_ANYWHERE_LEN`] bytes or more, is added with its own forms
    /// (not decoded again). A `%` not followed by two hex digits decodes to
    /// itself, so it adds nothing.
    pub fn add(&mut self, secret: &str) {
        let secret = secret.trim();
        if secret.is_empty() {
            return;
        }
        self.add_forms(secret);
        let decoded = percent_decode(secret);
        let decoded = decoded.trim();
        if decoded != secret && decoded.len() >= MIN_ANYWHERE_LEN {
            self.add_forms(decoded);
        }
    }

    /// The forms of [`SecretSet::add`] for one value, `secret` trimmed and
    /// not empty.
    fn add_forms(&mut self, secret: &str) {
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
        let hex: String = secret.bytes().map(|b| format!("{b:02x}")).collect();
        let mut forms = vec![
            secret.to_string(),
            hex,
            percent_encode(secret, false, false),
            percent_encode(secret, false, true),
            percent_encode(secret, true, false),
            percent_encode(secret, true, true),
            json_inner,
        ];
        forms.extend(url_serialized_forms(secret));
        // A base64 form of a very short secret is itself short and generic.
        if secret.len() >= 4 {
            forms.extend(engines);
        }
        let trimmed: Vec<String> = forms
            .iter()
            .map(|form| form.trim_end_matches('/'))
            .filter(|form| !form.is_empty())
            .map(str::to_string)
            .collect();
        forms.extend(trimmed);
        // A short form is matched case-sensitively (whole token): its escapes
        // are added in lower case too (`a:b%5Ec` echoed `a:b%5ec`).
        let lower: Vec<String> = forms
            .iter()
            .filter(|form| form.len() < MIN_ANYWHERE_LEN && form.contains('%'))
            .map(|form| lower_hex_escapes(form))
            .collect();
        forms.extend(lower);
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
                // ASCII case folded: covers upper-case hex, mixed-case
                // percent escapes and a raw value echoed in another case.
                out = replace_ascii_case_insensitive(&out, form);
            } else {
                out = replace_whole_tokens(&out, form);
            }
        }
        out
    }
}

fn replace_ascii_case_insensitive(text: &str, needle: &str) -> String {
    let haystack = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(found) = haystack[from..].find(&needle) {
        let index = from + found;
        out.push_str(&text[last..index]);
        out.push_str(MASK);
        last = index + needle.len();
        from = last;
    }
    out.push_str(&text[last..]);
    out
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

/// Mask `scheme://userinfo@host` credentials in a string so a PAT embedded in a
/// clone URL never leaks into an error message or a log line. Conservative:
/// collapses the characters between `://` and the next `@` (when there's no `/`
/// in between, i.e. it's really userinfo) to `***`.
pub fn redact_url_credentials(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("://") {
        out.push_str(&rest[..pos + 3]);
        let after = &rest[pos + 3..];
        // The userinfo terminator `@` must come before any `/` (path start).
        let at = after.find('@');
        let slash = after.find('/');
        match (at, slash) {
            (Some(a), maybe_slash) if maybe_slash.map(|sl| a < sl).unwrap_or(true) => {
                out.push_str("***");
                rest = &after[a..]; // keep `@host...`
            }
            _ => {
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(all(test, feature = "full-tests"))]
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
    fn hex_mixed_case_escapes_and_another_case_are_masked() {
        let mut set = SecretSet::new();
        set.add("Abc/Def+Secret9");
        let hex_upper: String = "Abc/Def+Secret9"
            .bytes()
            .map(|b| format!("{b:02X}"))
            .collect();
        let out = set.scrub(&format!(
            "{hex_upper} | Abc%2fDef%2BSecret9 | ABC/DEF+SECRET9 | {}",
            hex_upper.to_ascii_lowercase()
        ));
        assert!(!out.contains("Def"), "{out}");
        assert!(!out.contains("DEF"), "{out}");
        assert!(!out.contains(&hex_upper), "{out}");
    }

    /// Whether a half of `Abcdefgh{c}:ijklmnop` survives in `out`.
    fn holds_a_half(out: &str) -> bool {
        let folded = out.to_ascii_lowercase();
        folded.contains("abcdefgh") || folded.contains("ijklmnop")
    }

    /// Every printable ASCII character `c` in `Abcdefgh{c}:ijklmnop`, the
    /// secret written raw into `template` and parsed by `Url::parse`: no half
    /// of it may survive the scrub of the URL as the serializer writes it.
    /// Returns the leaks and the characters for which the value did not land
    /// in the URL on `api.example.com` at all (it does not parse, or it moved
    /// the host): the caller says which those may be.
    fn leaks_in_serialized_url(template: &str) -> (Vec<String>, String) {
        leaks_in_transformed_url(template, "Abcdefgh{c}:ijklmnop", |url| url)
    }

    /// [`leaks_in_serialized_url`] for the secret `shape` (`{c}` replaced by
    /// each character, its halves `Abcdefgh` and `ijklmnop`), the parsed URL
    /// first taken through `transform` (what the request does to it before
    /// it is sent).
    fn leaks_in_transformed_url(
        template: &str,
        shape: &str,
        transform: fn(reqwest::Url) -> reqwest::Url,
    ) -> (Vec<String>, String) {
        let mut leaks = Vec::new();
        let mut not_in_slot = String::new();
        for c in (0x20u8..=0x7e).map(char::from) {
            let secret = shape.replace("{c}", &c.to_string());
            let mut set = SecretSet::new();
            set.add(&secret);
            let url = match reqwest::Url::parse(&template.replace("{SECRET}", &secret)) {
                Ok(url) if url.host_str() == Some("api.example.com") => transform(url),
                _ => {
                    not_in_slot.push(c);
                    continue;
                }
            };
            let out = set.scrub(url.as_str());
            if holds_a_half(&out) {
                leaks.push(format!("{c:?}: sent {} scrubbed {out}", url.as_str()));
            }
        }
        (leaks, not_in_slot)
    }

    /// No character is skipped in a path, a query or a fragment: a `/`, `?`
    /// or `#` cuts the path or the query, and the value is then serialised
    /// partly with the next component's set, which the registered whole-tail
    /// form follows.
    fn assert_masked_everywhere(template: &str) {
        let (leaks, not_in_slot) = leaks_in_serialized_url(template);
        assert!(leaks.is_empty(), "{}", leaks.join("\n"));
        assert_eq!(not_in_slot, "", "{template}");
    }

    #[test]
    fn a_secret_written_raw_in_a_url_path_is_masked_as_url_sends_it() {
        assert_masked_everywhere("https://api.example.com/v1/bot{SECRET}/send");
        assert_masked_everywhere("https://api.example.com/v1/{SECRET}");
    }

    /// Glued on both sides, the value's dot segments are not resolved as
    /// they are when it stands alone: the glued probe covers it.
    #[test]
    fn a_secret_glued_inside_a_path_segment_is_masked_as_url_sends_it() {
        assert_masked_everywhere("https://api.example.com/v1/bot{SECRET}x/send");
        for secret in ["Abc^def12/..", "Abc^def12/.", "Abc^def12/%2e%2E"] {
            let mut set = SecretSet::new();
            set.add(secret);
            let url = reqwest::Url::parse(&format!("https://api.example.com/v1/bot{secret}x/send"))
                .unwrap();
            let out = set.scrub(url.as_str());
            assert!(
                !out.contains("def12"),
                "{secret}: sent {url} scrubbed {out}"
            );
        }
    }

    #[test]
    fn a_secret_written_raw_in_a_url_query_is_masked_as_url_sends_it() {
        assert_masked_everywhere("https://api.example.com/v1/x?key={SECRET}&z=1");
    }

    /// One character at a time, another registered form happens to match;
    /// a `{` (kept in a fragment, escaped in a path) with a backtick (escaped
    /// in a fragment, kept in a query) is matched by the fragment form only.
    #[test]
    fn a_secret_written_raw_in_a_url_fragment_is_masked_as_url_sends_it() {
        assert_masked_everywhere("https://api.example.com/v1/x#{SECRET}");
        let secret = "Abcd{efgh`ijkl";
        let mut set = SecretSet::new();
        set.add(secret);
        let url = reqwest::Url::parse(&format!("https://api.example.com/v1/x#{secret}")).unwrap();
        assert_eq!(url.fragment(), Some("Abcd{efgh%60ijkl"));
        assert_eq!(set.scrub(url.as_str()), "https://api.example.com/v1/x#***");
    }

    /// In user info, a `/`, `?`, `#` or `\` ends the authority: the value is
    /// then a host or a path, not user info (a host label has its own form,
    /// see [`host_label_form`]). As the whole user info, the secret's
    /// own `:` is the separator, so its first half is the user name.
    #[test]
    fn a_secret_written_raw_in_url_user_info_is_masked_as_url_sends_it() {
        for template in [
            "https://user:{SECRET}@api.example.com/v1",
            "https://{SECRET}@api.example.com/v1",
        ] {
            let (leaks, not_in_slot) = leaks_in_serialized_url(template);
            assert!(leaks.is_empty(), "{template}\n{}", leaks.join("\n"));
            assert_eq!(not_in_slot, "#/?\\", "{template}");
        }
        for secret in ["P@ss!word1", "p+ss:w0rd99"] {
            let mut set = SecretSet::new();
            set.add(secret);
            let url = reqwest::Url::parse(&format!("https://user:{secret}@h.example/v1")).unwrap();
            assert_eq!(set.scrub(url.as_str()), "https://user:***@h.example/v1");
        }
    }

    /// The same characters set through the crate's setters, which escape
    /// what `Url::parse` would read as structure.
    #[test]
    fn a_secret_set_as_a_url_user_name_or_password_is_masked() {
        let mut leaks = Vec::new();
        for c in (0x20u8..=0x7e).map(char::from) {
            let secret = format!("Abcdefgh{c}:ijklmnop");
            let mut set = SecretSet::new();
            set.add(&secret);
            let mut url = reqwest::Url::parse("https://api.example.com/v1").unwrap();
            url.set_username(&secret).unwrap();
            url.set_password(Some(&secret)).unwrap();
            let out = set.scrub(url.as_str());
            if holds_a_half(&out) {
                leaks.push(format!("{c:?}: sent {url} scrubbed {out}"));
            }
        }
        assert!(leaks.is_empty(), "{}", leaks.join("\n"));
    }

    #[test]
    fn percent_decode_decodes_valid_escapes_and_keeps_the_rest() {
        let cases = [
            ("", ""),
            ("%", "%"),
            ("%2", "%2"),
            ("a%2", "a%2"),
            ("%zz", "%zz"),
            ("%%41", "%A"),
            ("%C3%A9", "é"),
            ("%FF", "\u{FFFD}"),
            ("abc%5E", "abc^"),
            ("%5E", "^"),
            ("%5e%3a%3A", "^::"),
            ("a+b", "a+b"),
        ];
        for (serialized, decoded) in cases {
            assert_eq!(percent_decode(serialized), decoded, "{serialized:?}");
        }
    }

    /// KT-2007 — a pair with no `=` was skipped, so a bare query token
    /// (`?SecretTokenWithoutEquals`) was registered nowhere; an empty value
    /// (`k=`) still gives nothing.
    #[test]
    fn literal_query_values_count_a_pair_with_no_equals_as_a_value() {
        let url = reqwest::Url::parse("https://h.example/x?a=1&bare&=v&k=").unwrap();
        assert_eq!(literal_query_values(&url), vec!["1", "bare", "v"]);
        let url = reqwest::Url::parse("https://h.example/x?a=1&&b%5Ec#f").unwrap();
        assert_eq!(literal_query_values(&url), vec!["1", "b%5Ec"]);
        let url = reqwest::Url::parse("https://h.example/x").unwrap();
        assert!(literal_query_values(&url).is_empty());
    }

    /// KT-2007 — `;` is no separator for the parser: the first piece follows
    /// the caller's rule, a later piece is added from 8 bytes, each raw and
    /// decoded; a value with no `;` adds nothing.
    #[test]
    fn add_semicolon_pieces_adds_the_first_piece_and_long_later_ones() {
        let mut set = SecretSet::new();
        add_semicolon_pieces(
            &mut set,
            "AbC+dEf%5EghIJ==;v=1;Later%5Epiece9",
            MIN_ANYWHERE_LEN,
        );
        assert_eq!(
            set.scrub("t AbC+dEf%5EghIJ== / AbC+dEf^ghIJ== v=1 Later^piece9 Later%5Epiece9"),
            "t *** / *** v=1 *** ***"
        );
        let mut short = SecretSet::new();
        add_semicolon_pieces(&mut short, "ab;v=1", 0);
        assert_eq!(short.scrub("ab v=1 abc"), "*** v=1 abc");
        let mut long_rule = SecretSet::new();
        add_semicolon_pieces(&mut long_rule, "ab;v=1", MIN_ANYWHERE_LEN);
        assert!(long_rule.is_empty());
        let mut none = SecretSet::new();
        add_semicolon_pieces(&mut none, "NoSemicolonHere99", 0);
        assert!(none.is_empty());
    }

    /// KT-2007 — a later piece that is itself `key=value`
    /// (`v=1;token=AbC%5EdEf12345`) was registered whole: the token echoed
    /// alone, raw or decoded, was not masked. What follows its `=` is added
    /// from 8 bytes, so `w=2` and its `2` are not.
    #[test]
    fn add_semicolon_pieces_adds_the_value_of_a_later_key_value_piece() {
        let mut set = SecretSet::new();
        add_semicolon_pieces(&mut set, "1;token=AbC%5EdEf12345;w=2", MIN_ANYWHERE_LEN);
        assert_eq!(
            set.scrub(r#"invalid token AbC%5EdEf12345 (AbC^dEf12345) {"v":1,"w=2":2}"#),
            r#"invalid token *** (***) {"v":1,"w=2":2}"#
        );
    }

    /// KT-2007 — a bare query token is handed back by `query_pairs()` as a
    /// key, form-decoded (`+` read as a space); a pair with `=`, an empty
    /// pair and an empty key give nothing.
    #[test]
    fn bare_query_tokens_are_the_form_decoded_keys_of_pairs_with_no_equals() {
        let url =
            reqwest::Url::parse("https://h.example/x?a=1&AbC+dEf/ghIJkl&&b%5Ec&=v&k=#f").unwrap();
        assert_eq!(bare_query_tokens(&url), vec!["AbC dEf/ghIJkl", "b^c"]);
        let url = reqwest::Url::parse("https://h.example/x").unwrap();
        assert!(bare_query_tokens(&url).is_empty());
    }

    /// KT-2007 — `Authorization: Basic` decodes to the password and
    /// `user:password`, the scheme matched in any ASCII case; another scheme
    /// adds nothing.
    #[test]
    fn add_basic_authorization_adds_the_decoded_credentials_in_any_case() {
        let encoded = base64::engine::general_purpose::STANDARD.encode("hookuser:Pa^ssword7");
        for scheme in ["Basic", "basic", "bAsIc"] {
            let mut set = SecretSet::new();
            add_basic_authorization(&mut set, &format!("{scheme} {encoded}"));
            assert_eq!(
                set.scrub(&format!("bad Pa^ssword7 / hookuser:Pa^ssword7 / {encoded}")),
                "bad *** / *** / ***",
                "{scheme}"
            );
        }
        let mut other = SecretSet::new();
        add_basic_authorization(&mut other, &format!("Bearer {encoded}"));
        assert!(other.is_empty());
    }

    /// KT-2007 — `strip_query_keys` (cross-origin redirect) and link-header
    /// pagination read the pairs back with `query_pairs()` and write them
    /// again with `query_pairs_mut()`: the value is then sent decoded as a
    /// form and form-encoded again, a form no other probe gives. Every
    /// character lands in the URL on the host; a `&` or `=` splits the value
    /// into pairs and a `#` moves its tail (and `&z=1`) into the fragment,
    /// which the whole-tail form still matches.
    ///
    /// For most values another form happens to be the same string (the
    /// form-encoded value when it holds no `+` or `%`, the password form for
    /// `AbC+dEf/ghIJ==`): the second shape, a `+` (form-encoded `%2B`) next
    /// to a `!` (kept in user info, `%21` in a form), is matched by the
    /// round-trip form only.
    #[test]
    fn a_secret_in_a_query_read_back_and_written_again_is_masked() {
        fn round_trip(mut url: reqwest::Url) -> reqwest::Url {
            let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
            url.query_pairs_mut().clear().extend_pairs(pairs);
            url
        }
        for shape in ["Abcdefgh{c}:ijklmnop", "Abcdefgh+{c}!ijklmnop"] {
            let (leaks, not_in_slot) = leaks_in_transformed_url(
                "https://api.example.com/v1/x?q={SECRET}&z=1",
                shape,
                round_trip,
            );
            assert!(leaks.is_empty(), "{shape}\n{}", leaks.join("\n"));
            assert_eq!(not_in_slot, "", "{shape}");
        }
        for (secret, sent) in [
            ("AbC+dEf/ghIJ==", "AbC+dEf%2FghIJ%3D%3D"),
            ("Abc%20def:ghi", "Abc+def%3Aghi"),
            ("AbC+dEf!ghIJ==", "AbC+dEf%21ghIJ%3D%3D"),
        ] {
            let mut set = SecretSet::new();
            set.add(secret);
            let url = round_trip(
                reqwest::Url::parse(&format!("https://h.example/x?q={secret}&z=1")).unwrap(),
            );
            assert_eq!(url.query(), Some(format!("q={sent}&z=1").as_str()));
            assert_eq!(set.scrub(url.as_str()), "https://h.example/x?q=***&z=1");
        }
    }

    /// KT-2007 — `build_url` trims the trailing slashes of a base URL: a
    /// secret ending in `/` at its end is sent without them.
    #[test]
    fn a_secret_ending_in_slashes_is_masked_without_them() {
        let mut set = SecretSet::new();
        set.add("Kq7xBase^Url:Secret42//");
        assert_eq!(
            set.scrub("GET https://h.example/v1/Kq7xBase%5EUrl:Secret42/items"),
            "GET https://h.example/v1/***/items"
        );
    }

    /// KT-2007 — a secret stored pre-encoded is echoed decoded by a server.
    #[test]
    fn a_pre_encoded_secret_is_masked_decoded() {
        let mut set = SecretSet::new();
        set.add("Abc%5Edef%3Aghi");
        assert_eq!(
            set.scrub("echo Abc^def:ghi and Abc%5Edef%3Aghi"),
            "echo *** and ***"
        );
    }

    /// A `%` not followed by two hex digits decodes to itself: such a
    /// secret registers exactly its own forms, nothing decoded.
    #[test]
    fn a_secret_with_a_bare_percent_registers_nothing_decoded() {
        for secret in ["Abc%zzdef%4", "100%Secret-key", "Abcdefg%"] {
            let mut set = SecretSet::new();
            set.add(secret);
            let mut own = SecretSet::new();
            own.add_forms(secret);
            assert_eq!(set.forms, own.forms, "{secret}");
        }
    }

    #[test]
    fn a_telegram_style_token_with_a_caret_is_masked_in_its_sent_path_form() {
        let mut set = SecretSet::new();
        set.add("bot123456:AbC^dEfSecret");
        let sent = "https://api.telegram.org/bot123456:AbC%5EdEfSecret/sendMessage";
        assert_eq!(set.scrub(sent), "https://api.telegram.org/***/sendMessage");
    }

    /// KT-2007 — a form under 8 bytes is matched case-sensitively: the path
    /// form `a:b%5Ec` of a 5-byte secret, echoed with lower-case hex, was
    /// not masked.
    #[test]
    fn a_short_form_is_masked_with_its_escapes_in_lower_case() {
        let mut set = SecretSet::new();
        set.add("a:b^c");
        assert_eq!(
            set.scrub("sent a:b%5Ec echoed a:b%5ec glued xa:b%5ecx"),
            "sent *** echoed *** glued xa:b%5ecx"
        );
        assert_eq!(lower_hex_escapes("A%5EB%3a%zz%4"), "A%5eB%3a%zz%4");
    }

    /// KT-2007 — a non-ASCII value interpolated in the host is sent in its
    /// IDNA form, which no other form matches.
    #[test]
    fn a_non_ascii_secret_used_as_a_host_label_is_masked_in_its_idna_form() {
        let secret = "Tenänt-Gëheim7";
        let mut set = SecretSet::new();
        set.add(secret);
        let url = reqwest::Url::parse(&format!("https://{secret}.example.com/v1")).unwrap();
        let label = url
            .host_str()
            .unwrap()
            .strip_suffix(".example.com")
            .unwrap();
        assert!(label.starts_with("xn--"), "{url}");
        assert_eq!(set.scrub(url.as_str()), "https://***.example.com/v1");
    }

    /// An ordinary ASCII value is its own host label (lower-cased): nothing
    /// new is registered; a value that does not land whole in the host gives
    /// no form either.
    #[test]
    fn an_ascii_secret_adds_no_host_label_form() {
        for secret in ["AbcTenant42", "tenant-acme", "x9.y8.z7"] {
            assert_eq!(host_label_form(secret), None, "{secret}");
        }
        for secret in [
            "user@tenant",
            "a/b",
            "a?b",
            "a#b",
            "a:1",
            "%7B%7D",
            "Abc%41",
        ] {
            assert_eq!(host_label_form(secret), None, "{secret}");
        }
        let idna = host_label_form("Tenänt").expect("IDNA form");
        assert!(idna.starts_with("xn--") && idna.is_ascii(), "{idna}");
    }

    #[test]
    fn a_short_secret_is_masked_only_as_a_whole_token() {
        let mut set = SecretSet::new();
        set.add("4821");
        assert_eq!(set.scrub("pin=4821, id 148210"), "pin=***, id 148210");
    }
}
