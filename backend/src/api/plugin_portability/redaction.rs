//! KT-1040 — what a configuration-only export may carry of a plugin. An
//! allow-list: a leaf leaves the machine only when it is provably not a secret
//! (a catalogue default, a reference, a name, a URL origin…). Every other
//! value becomes a named placeholder, `${ENV.KRONN_EXPORT_<FIELD>}`, which the
//! import lists and asks the owner to fill. A leaf with no rule is withheld.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::models::McpServer;

/// Env-key prefix of every placeholder an export writes.
pub const PLACEHOLDER_PREFIX: &str = "KRONN_EXPORT_";

/// The JSON type of a withheld value, so the typed value goes back as one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ExportValueType {
    #[default]
    Text,
    Number,
    Boolean,
}

/// One value a configuration-only export withheld.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportPlaceholder {
    /// The env key the placeholder names: the definition holds `${ENV.<key>}`.
    pub key: String,
    /// Where the value stood, e.g. `api_spec.base_url`.
    pub field: String,
    /// The plugin cannot work without it (a URL, a header, an argument…);
    /// free text is optional.
    pub required: bool,
    /// What may be shown of the withheld value: a URL's origin, nothing else.
    pub hint: Option<String>,
    #[serde(default)]
    pub value_type: ExportValueType,
}

/// How a leaf of the definition is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    /// A server id: kept when it is a catalogue id or Kronn's own
    /// `custom-<slug>-<8 hex>`, else replaced by a fresh one (never a placeholder).
    Id,
    /// An env-key name: kept only as `[A-Za-z0-9_]+` with no token-shaped word.
    EnvKey,
    /// An enum tag or an HTTP method: kept only as a short word of letters.
    Tag,
    /// A name, a label, a header name, a scope, a JSONPath: kept only when no
    /// word of it is token-shaped.
    Name,
    /// A number or a boolean the definition declares (a TTL, a flag).
    Scalar,
    /// Kept only when made of `${ENV.KEY}` references (around an auth scheme word).
    EnvReference,
    /// Kept only when made of `{KEY}` references (OAuth2 extra headers).
    BraceReference,
    /// Kept only as `scheme://host[:port]` with a clean host, or a lone reference.
    Url,
    /// Kept only when every segment is a reference, `vN` or a plain lowercase word.
    RelativePath,
    /// A command-line argument: kept only as `-x` or `--long-name`, or a reference.
    Argument,
    /// Kept only when equal to the catalogue's.
    Catalogue,
    /// Descriptive text: withheld, optional to fill.
    FreeText,
    /// No decision: withheld.
    Unknown,
}

/// Leaves of a TokenExchange request body: references kept, any literal withheld.
const BODY: &str = "api_spec.auth.TokenExchange.body_template";

/// Every decided leaf path, array indices written `[]`.
pub(crate) const RULES: &[(&str, Rule)] = &[
    ("id", Rule::Id),
    ("name", Rule::Name),
    ("label", Rule::Name),
    ("env_keys[]", Rule::EnvKey),
    ("description", Rule::FreeText),
    ("source", Rule::Tag),
    ("transport", Rule::Tag),
    ("transport.Stdio.command", Rule::Catalogue),
    ("transport.Stdio.args[]", Rule::Argument),
    ("transport.Sse.url", Rule::Url),
    ("transport.Streamable.url", Rule::Url),
    ("api_spec.base_url", Rule::Url),
    ("api_spec.docs_url", Rule::Url),
    ("api_spec.test_endpoint", Rule::RelativePath),
    ("api_spec.endpoints[].path", Rule::RelativePath),
    ("api_spec.endpoints[].method", Rule::Tag),
    ("api_spec.endpoints[].description", Rule::FreeText),
    ("api_spec.config_keys[].env_key", Rule::EnvKey),
    ("api_spec.config_keys[].label", Rule::Name),
    ("api_spec.config_keys[].placeholder", Rule::FreeText),
    ("api_spec.config_keys[].description", Rule::FreeText),
    ("api_spec.config_keys[].public", Rule::Scalar),
    ("api_spec.default_headers[].name", Rule::Name),
    ("api_spec.default_headers[].value", Rule::EnvReference),
    ("api_spec.auth", Rule::Tag),
    ("api_spec.auth.ApiKeyQuery.param_name", Rule::Name),
    ("api_spec.auth.ApiKeyQuery.env_key", Rule::EnvKey),
    ("api_spec.auth.ApiKeyHeader.header_name", Rule::Name),
    ("api_spec.auth.ApiKeyHeader.env_key", Rule::EnvKey),
    ("api_spec.auth.Bearer.env_key", Rule::EnvKey),
    ("api_spec.auth.Basic.user_env", Rule::EnvKey),
    ("api_spec.auth.Basic.password_env", Rule::EnvKey),
    ("api_spec.auth.BasicApiKey.env_key", Rule::EnvKey),
    ("api_spec.auth.CliToken.command", Rule::Catalogue),
    ("api_spec.auth.CliToken.args[]", Rule::Catalogue),
    ("api_spec.auth.CliToken.inject", Rule::Tag),
    (
        "api_spec.auth.CliToken.inject.CustomHeader.name",
        Rule::Name,
    ),
    ("api_spec.auth.CliToken.inject.QueryParam.name", Rule::Name),
    ("api_spec.auth.CliToken.fallback_env_key", Rule::EnvKey),
    ("api_spec.auth.OAuth2ClientCredentials.token_url", Rule::Url),
    (
        "api_spec.auth.OAuth2ClientCredentials.client_id_env",
        Rule::EnvKey,
    ),
    (
        "api_spec.auth.OAuth2ClientCredentials.client_secret_env",
        Rule::EnvKey,
    ),
    ("api_spec.auth.OAuth2ClientCredentials.scope", Rule::Name),
    (
        "api_spec.auth.OAuth2ClientCredentials.extra_headers[].name",
        Rule::Name,
    ),
    (
        "api_spec.auth.OAuth2ClientCredentials.extra_headers[].value_template",
        Rule::BraceReference,
    ),
    ("api_spec.auth.TokenExchange.endpoint", Rule::RelativePath),
    ("api_spec.auth.TokenExchange.method", Rule::Tag),
    (BODY, Rule::EnvReference),
    ("api_spec.auth.TokenExchange.body_format", Rule::Tag),
    ("api_spec.auth.TokenExchange.token_jsonpath", Rule::Name),
    ("api_spec.auth.TokenExchange.ttl_seconds", Rule::Scalar),
    ("api_spec.auth.TokenExchange.inject", Rule::Tag),
    (
        "api_spec.auth.TokenExchange.inject.CustomHeader.name",
        Rule::Name,
    ),
    (
        "api_spec.auth.TokenExchange.inject.QueryParam.name",
        Rule::Name,
    ),
    ("api_spec.auth.TokenExchange.creds_env_keys[]", Rule::EnvKey),
];

/// The rule of a leaf, by its path with array indices written `[]`.
pub(crate) fn rule(pattern: &str) -> Rule {
    if pattern.starts_with(BODY) {
        return Rule::EnvReference;
    }
    RULES
        .iter()
        .find(|(path, _)| *path == pattern)
        .map_or(Rule::Unknown, |(_, rule)| *rule)
}

/// A word of a name that could be a credential: letters mixed with digits
/// over 8 characters (`OAuth2`, `S3`, `GPT4`, `K8s` are names), a run of six
/// digits or more, or longer than 20 characters.
fn token_like(word: &str) -> bool {
    mixed(word) && word.chars().count() > 8 || shape_suspect(word)
}

/// The stricter rule of a URL host label: any letter-digit mix, a version aside.
fn host_label_suspect(label: &str) -> bool {
    (mixed(label) && !is_version(&label.to_ascii_lowercase())) || shape_suspect(label)
}

fn mixed(word: &str) -> bool {
    word.chars().any(|c| c.is_numeric()) && word.chars().any(|c| c.is_alphabetic())
}

/// Six digits in a row or more, or longer than 20 characters.
fn shape_suspect(word: &str) -> bool {
    let mut run = 0;
    let mut longest = 0;
    for c in word.chars() {
        run = if c.is_numeric() { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    word.chars().count() > 20 || longest >= 6
}

pub(crate) fn name_allowed(name: &str) -> bool {
    name.chars().count() <= 80
        && name
            .split(|c: char| !c.is_alphanumeric())
            .all(|word| !token_like(word))
        && !segmented_token(name)
}

/// A token cut into segments, whatever separates them (`4B7D-9F1C-0A2B`,
/// `4B7D.9F1C`, `4111 1111 1111 1111`): over 8 characters in all, two
/// segments or more holding a digit. `OAUTH2_CLIENT_ID` or `GPT4 Turbo` have
/// one such segment and stay names.
fn segmented_token(value: &str) -> bool {
    let segments: Vec<&str> = value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .collect();
    value.trim().chars().count() > 8
        && segments
            .iter()
            .filter(|segment| segment.chars().any(|c| c.is_numeric()))
            .count()
            >= 2
}

fn tag_allowed(tag: &str) -> bool {
    (1..=32).contains(&tag.len()) && tag.chars().all(|c| c.is_ascii_alphabetic())
}

fn is_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// An env-key name an export may carry: a key with no token-shaped word.
/// A key derived from a withheld label would otherwise bring it back.
pub(crate) fn env_key_allowed(key: &str) -> bool {
    is_key(key) && name_allowed(key)
}

/// The slug of Kronn's own id for a custom plugin, `custom-<slug>-<8 hex>`.
fn kronn_custom_slug(id: &str) -> Option<&str> {
    let (slug, nano) = id.strip_prefix("custom-")?.rsplit_once('-')?;
    let shaped = nano.len() == 8
        && nano
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && !slug.is_empty()
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    shaped.then_some(slug)
}

/// An id is kept when it carries nothing the name check did not: Kronn's own
/// format whose slug is the slug of a KEPT name (the slug is the lowercased
/// name, so it would bring a withheld one back), or a plain slug that passes
/// the name check itself.
fn id_allowed(id: &str, kept_name_slug: Option<&str>) -> bool {
    match kronn_custom_slug(id) {
        Some(slug) => kept_name_slug == Some(slug),
        None => tag_or_slug(id) && name_allowed(id),
    }
}

fn tag_or_slug(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c))
}

/// A fresh id in Kronn's custom format, for a definition whose id is
/// withheld. The same within one export (`salt`), so configs of one server
/// keep sharing it; unrelated to the id across exports.
fn fresh_id(salt: &str, id: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{salt}\0{id}").as_bytes());
    let hex: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("custom-export-{hex}")
}

/// `text` with every `open…}` reference removed, or `None` when a reference is
/// malformed or its key could be a secret. Also returns how many were removed.
fn strip_references(
    text: &str,
    open: &str,
    declared: &BTreeSet<String>,
) -> Option<(String, usize)> {
    let mut literal = String::new();
    let mut rest = text;
    let mut count = 0;
    loop {
        let start = if open == "${" {
            rest.to_ascii_lowercase().find("${env.")
        } else {
            rest.find('{')
        };
        let Some(start) = start else { break };
        let skip = if open == "${" { 6 } else { 1 };
        let length = rest[start..].find('}')?;
        let key = &rest[start + skip..start + length];
        if !(env_key_allowed(key) || placeholder_key(key) || declared.contains(key)) {
            return None;
        }
        literal.push_str(&rest[..start]);
        rest = &rest[start + length + 1..];
        count += 1;
    }
    literal.push_str(rest);
    Some((literal, count))
}

/// Made of references only, optionally after an auth scheme word.
fn references_only(text: &str, open: &str, declared: &BTreeSet<String>) -> bool {
    let Some((literal, count)) = strip_references(text, open, declared) else {
        return false;
    };
    let word: String = literal
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
        .collect::<String>()
        .to_ascii_lowercase();
    count > 0 && ["", "bearer", "basic", "token", "bot"].contains(&word.as_str())
}

fn lone_reference(text: &str) -> bool {
    let inner = text
        .strip_prefix("${ENV.")
        .or_else(|| text.strip_prefix("${env."))
        .or_else(|| text.strip_prefix('{'))
        .and_then(|rest| rest.strip_suffix('}'));
    inner.is_some_and(env_key_allowed)
}

/// `scheme://host[:port]` of an http(s) URL, user info dropped.
pub(crate) fn url_origin(url: &str) -> Option<String> {
    let (scheme, host, port) = url_parts(url)?;
    Some(match port {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Scheme, host and numeric port of an http(s) URL, user info dropped. A
/// bracketed host must be a real IPv6 literal.
fn url_parts(url: &str) -> Option<(&str, &str, Option<&str>)> {
    let (scheme, rest) = url.split_once("://")?;
    if !["http", "https"].contains(&scheme.to_ascii_lowercase().as_str()) {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let close = bracketed.find(']')?;
        bracketed[..close].parse::<std::net::Ipv6Addr>().ok()?;
        let port = host_port[close + 2..].strip_prefix(':');
        if port.is_none() && close + 2 != host_port.len() {
            return None;
        }
        (&host_port[..close + 2], port)
    } else {
        match host_port.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    let host_ok = !host.is_empty()
        && (host.starts_with('[')
            || host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)));
    let port_ok = port.is_none_or(|port| {
        (1..=5).contains(&port.len()) && port.chars().all(|c| c.is_ascii_digit())
    });
    (host_ok && port_ok).then_some((scheme, host, port))
}

/// The origin an export may show: its host is an IP literal, has no
/// token-shaped label, or is the catalogue's.
fn safe_origin(url: &str, catalogue: Option<&str>) -> Option<String> {
    let (_, host, _) = url_parts(url)?;
    let catalogue_host = catalogue.and_then(url_parts).map(|(_, host, _)| host);
    let clean = host.starts_with('[')
        || host
            .split(['.', '-'])
            .all(|label| !host_label_suspect(label));
    (clean || catalogue_host == Some(host))
        .then(|| url_origin(url))
        .flatten()
}

/// A URL as a review may show it: whole when an export would keep it, else its
/// origin when the host is kept.
pub(crate) fn url_for_display(url: &str) -> String {
    if url_allowed(url, None) || is_placeholder(url) {
        url.to_string()
    } else {
        safe_origin(url, None).map_or_else(|| "…".to_string(), |origin| format!("{origin}/…"))
    }
}

fn url_allowed(url: &str, catalogue: Option<&str>) -> bool {
    if lone_reference(url) {
        return true;
    }
    let bare = url.strip_suffix('/').unwrap_or(url);
    safe_origin(bare, catalogue).is_some_and(|origin| origin == bare)
}

fn path_allowed(path: &str) -> bool {
    !path.contains(['?', '#'])
        && path.split('/').all(|segment| {
            segment.is_empty()
                || lone_reference(segment)
                || is_version(segment)
                || (segment.len() <= 32
                    && segment.starts_with(|c: char| c.is_ascii_lowercase())
                    && segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || "._-".contains(c)))
        })
}

fn is_version(segment: &str) -> bool {
    segment.strip_prefix('v').is_some_and(|rest| {
        (1..=3).contains(&rest.len()) && rest.chars().all(|c| c.is_ascii_digit())
    })
}

/// `-x` (one letter) or `--long-name` (lowercase words): a flag, never its
/// value; `-pS3cret` and dash-led tokens are not flags.
fn argument_allowed(arg: &str) -> bool {
    if lone_reference(arg) {
        return true;
    }
    if let Some(long) = arg.strip_prefix("--") {
        return (1..=40).contains(&long.len())
            && long.starts_with(|c: char| c.is_ascii_lowercase())
            && long.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    }
    arg.len() == 2 && arg.starts_with('-') && arg[1..].chars().all(|c| c.is_ascii_alphabetic())
}

/// The one test of a placeholder key: the prefix and the name check, so the
/// prefix can never carry a secret through.
pub(crate) fn placeholder_key(key: &str) -> bool {
    key.starts_with(PLACEHOLDER_PREFIX) && env_key_allowed(key)
}

/// Exactly one export placeholder, for the checks outside this module.
pub(crate) fn is_export_placeholder(text: &str) -> bool {
    is_placeholder(text)
}

/// Exactly one export placeholder.
fn is_placeholder(text: &str) -> bool {
    text.strip_prefix("${ENV.")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(placeholder_key)
}

fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_uppercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    // A placeholder key passes the name check itself: token-shaped words of a
    // field path (`OAuth2ClientCredentials`) keep their letters, cut to 20.
    let out = out
        .trim_matches('_')
        .split('_')
        .map(|word| {
            if token_like(word) {
                let letters: String = word
                    .chars()
                    .filter(char::is_ascii_alphabetic)
                    .take(20)
                    .collect();
                if letters.is_empty() {
                    "N".to_string()
                } else {
                    letters
                }
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("_");
    // Two numbered segments (`ARGS_0_..._1`) would read as a cut token: the
    // indices become letters (0 → A), uniqueness is kept by the caller.
    if name_allowed(&format!("{PLACEHOLDER_PREFIX}{out}")) {
        out
    } else {
        out.split('_')
            .map(|word| {
                word.chars()
                    .map(|c| {
                        c.to_digit(10)
                            .map_or(c, |digit| (b'A' + digit as u8) as char)
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("_")
    }
}

fn join(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_string()
    } else {
        format!("{base}.{key}")
    }
}

pub(crate) struct Walk<'a> {
    pub(crate) catalogue: Option<&'a Value>,
    /// Env keys the catalogue entry declares, or the definition kept in clear.
    pub(crate) declared: BTreeSet<String>,
    /// Makes a regenerated id stable within one export.
    pub(crate) salt: String,
    /// `name_slug` of the definition's name when the export keeps that name.
    pub(crate) kept_name_slug: Option<String>,
    pub(crate) placeholders: Vec<ExportPlaceholder>,
    /// The same literal gets the same placeholder, so equal fields stay equal.
    pub(crate) by_value: BTreeMap<String, usize>,
    /// Leaf paths no rule decides (withheld all the same).
    pub(crate) unknown: Vec<String>,
}

impl Walk<'_> {
    pub(crate) fn new(catalogue: Option<&Value>) -> Walk<'_> {
        Walk {
            catalogue,
            declared: BTreeSet::new(),
            salt: uuid::Uuid::new_v4().simple().to_string(),
            kept_name_slug: None,
            placeholders: Vec::new(),
            by_value: BTreeMap::new(),
            unknown: Vec::new(),
        }
    }

    fn withhold(
        &mut self,
        value: &str,
        label: &str,
        rule: Rule,
        value_type: ExportValueType,
    ) -> String {
        let by = format!("{value_type:?}:{value}");
        if let Some(&index) = self.by_value.get(&by) {
            let placeholder = &mut self.placeholders[index];
            placeholder.required |= rule != Rule::FreeText;
            return format!("${{ENV.{}}}", placeholder.key);
        }
        let mut key = format!("{PLACEHOLDER_PREFIX}{}", slug(label));
        while self.placeholders.iter().any(|p| p.key == key) {
            key.push('_');
        }
        // Only an origin whose host could not be a secret is shown.
        let hint = (rule == Rule::Url)
            .then(|| safe_origin(value, None))
            .flatten();
        self.placeholders.push(ExportPlaceholder {
            key: key.clone(),
            field: label.to_string(),
            required: rule != Rule::FreeText,
            hint,
            value_type,
        });
        self.by_value.insert(by, self.placeholders.len() - 1);
        format!("${{ENV.{key}}}")
    }

    fn string(
        &mut self,
        text: &str,
        pattern: &str,
        label: &str,
        catalogue: Option<&Value>,
    ) -> Value {
        let rule = rule(pattern);
        if matches!(rule, Rule::Unknown | Rule::Scalar) {
            self.unknown.push(pattern.to_string());
        }
        let catalogue = catalogue.and_then(Value::as_str);
        if rule == Rule::Id {
            let kept = catalogue == Some(text) || id_allowed(text, self.kept_name_slug.as_deref());
            return Value::String(if kept {
                text.to_string()
            } else {
                fresh_id(&self.salt, text)
            });
        }
        // An empty string withholds nothing.
        let kept = text.is_empty()
            || catalogue == Some(text)
            || is_placeholder(text)
            || match rule {
                Rule::EnvKey => env_key_allowed(text) || self.declared.contains(text),
                Rule::Tag => tag_allowed(text),
                Rule::Name => name_allowed(text),
                Rule::EnvReference => references_only(text, "${", &self.declared),
                Rule::BraceReference => references_only(text, "{", &self.declared),
                Rule::Url => url_allowed(text, catalogue),
                Rule::RelativePath => path_allowed(text),
                Rule::Argument => argument_allowed(text),
                Rule::Id | Rule::Scalar | Rule::Catalogue | Rule::FreeText | Rule::Unknown => false,
            };
        if kept {
            if is_placeholder(text) {
                self.relist(text, label, rule);
            } else if rule == Rule::EnvKey {
                // Kept in the definition: the env-key list may name it too.
                self.declared.insert(text.to_string());
            }
            Value::String(text.to_string())
        } else {
            Value::String(self.withhold(text, label, rule, ExportValueType::Text))
        }
    }

    /// A placeholder already in the definition (a re-export) stays listed.
    fn relist(&mut self, text: &str, label: &str, rule: Rule) {
        let key = &text[6..text.len() - 1];
        if !self.placeholders.iter().any(|p| p.key == key) {
            self.placeholders.push(ExportPlaceholder {
                key: key.to_string(),
                field: label.to_string(),
                required: rule != Rule::FreeText,
                hint: None,
                value_type: ExportValueType::Text,
            });
        }
    }

    pub(crate) fn value(
        &mut self,
        value: &Value,
        pattern: &str,
        label: &str,
        catalogue: Option<&Value>,
    ) -> Value {
        match value {
            Value::String(text) => self.string(text, pattern, label, catalogue),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        self.value(
                            item,
                            &format!("{pattern}[]"),
                            &format!("{label}[{index}]"),
                            catalogue.and_then(|c| c.get(index)),
                        )
                    })
                    .collect(),
            ),
            Value::Object(map) => {
                // A request body's own keys are data too: shape-checked like names.
                let body = pattern.starts_with(BODY);
                Value::Object(
                    map.iter()
                        .enumerate()
                        .map(|(index, (key, item))| {
                            let kept = !body
                                || name_allowed(key)
                                || catalogue.and_then(|c| c.get(key)).is_some();
                            // A withheld key never names its placeholder nor its value's.
                            let segment = if kept {
                                key.clone()
                            } else {
                                format!("field_{index}")
                            };
                            let redacted = self.value(
                                item,
                                &join(pattern, key),
                                &join(label, &segment),
                                catalogue.and_then(|c| c.get(key)),
                            );
                            let key = if kept {
                                key.clone()
                            } else {
                                let name = join(&join(label, &segment), "name");
                                self.withhold(key, &name, Rule::Name, ExportValueType::Text)
                            };
                            (key, redacted)
                        })
                        .collect(),
                )
            }
            Value::Null => Value::Null,
            scalar => {
                let rule = rule(pattern);
                if rule == Rule::Unknown {
                    self.unknown.push(pattern.to_string());
                }
                if (rule == Rule::Scalar && !pattern.starts_with(BODY)) || catalogue == Some(value)
                {
                    return scalar.clone();
                }
                // A number or a flag no rule keeps (a body PIN, a new field) is withheld, typed.
                let value_type = if scalar.is_boolean() {
                    ExportValueType::Boolean
                } else {
                    ExportValueType::Number
                };
                Value::String(self.withhold(
                    &scalar.to_string(),
                    label,
                    Rule::EnvReference,
                    value_type,
                ))
            }
        }
    }
}

/// What a configuration-only export carries of one plugin.
pub(crate) struct Redacted {
    pub server: McpServer,
    pub label: String,
    pub env_keys: Vec<String>,
    pub placeholders: Vec<ExportPlaceholder>,
    /// Leaf paths no rule decided: empty unless a field was added without a decision.
    pub unknown: Vec<String>,
}

/// What one export knows besides the definition: the catalogue entry and the
/// env keys it declares, and the salt that keeps regenerated ids stable.
pub(crate) struct ExportContext<'a> {
    pub catalogue: Option<&'a McpServer>,
    pub declared_env_keys: &'a [String],
    pub salt: &'a str,
}

/// One walk over the definition, the configuration label and the env-key
/// names, so a literal shared between them gets one placeholder.
pub(crate) fn redact_plugin(
    server: &McpServer,
    context: &ExportContext<'_>,
    label: &str,
    env_keys: &[String],
) -> Redacted {
    let original = serde_json::to_value(server).unwrap_or(Value::Null);
    let catalogue = context.catalogue.and_then(|c| serde_json::to_value(c).ok());
    let mut walk = Walk::new(catalogue.as_ref());
    walk.salt = context.salt.to_string();
    walk.declared
        .extend(context.declared_env_keys.iter().cloned());
    // Decided as the walk decides a name, before the id that may derive from it.
    let name_kept = server.name.is_empty()
        || context.catalogue.is_some_and(|c| c.name == server.name)
        || name_allowed(&server.name);
    walk.kept_name_slug = name_kept.then(|| crate::api::mcps::name_slug(&server.name));
    let reference = walk.catalogue;
    let redacted = walk.value(&original, "", "", reference);
    let server = serde_json::from_value(redacted).unwrap_or_else(|_| McpServer {
        // A leaf that cannot carry a placeholder: nothing of the definition goes.
        id: fresh_id(context.salt, &server.id),
        name: String::new(),
        description: String::new(),
        transport: crate::models::McpTransport::ApiOnly,
        source: crate::models::McpSource::Manual,
        api_spec: None,
    });
    // A catalogue plugin's own name is no secret, nor a label equal to it.
    let catalogue_name = context.catalogue.map(|c| Value::String(c.name.clone()));
    let label = match walk.string(label, "label", "label", catalogue_name.as_ref()) {
        Value::String(label) => label,
        _ => String::new(),
    };
    let env_keys = env_keys
        .iter()
        .enumerate()
        .map(|(index, key)| {
            match walk.string(key, "env_keys[]", &format!("env_keys[{index}]"), None) {
                Value::String(key) => key,
                _ => String::new(),
            }
        })
        .collect();
    Redacted {
        server,
        label,
        env_keys,
        placeholders: walk.placeholders,
        unknown: walk.unknown,
    }
}

pub(crate) fn redact_server(
    server: &McpServer,
    catalogue: Option<&McpServer>,
) -> (McpServer, Vec<ExportPlaceholder>) {
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let context = ExportContext {
        catalogue,
        declared_env_keys: &[],
        salt: &salt,
    };
    let redacted = redact_plugin(server, &context, "", &[]);
    (redacted.server, redacted.placeholders)
}

fn placeholder_keys(text: &str, keys: &mut BTreeSet<String>) {
    let mut rest = text;
    while let Some(start) = rest.find("${ENV.") {
        let after = &rest[start + 6..];
        let Some(end) = after.find('}') else { break };
        let key = &after[..end];
        if placeholder_key(key) {
            keys.insert(key.to_string());
        }
        rest = &after[end + 1..];
    }
}

fn each_string(value: &mut Value, apply: &mut dyn FnMut(&mut String)) {
    match value {
        Value::String(text) => apply(text),
        Value::Array(items) => items.iter_mut().for_each(|item| each_string(item, apply)),
        Value::Object(map) => {
            // Object keys of a request body may be placeholders too.
            let entries = std::mem::take(map);
            for (mut key, mut item) in entries {
                apply(&mut key);
                each_string(&mut item, apply);
                map.insert(key, item);
            }
        }
        _ => {}
    }
}

/// A leaf that is exactly a typed placeholder, filled, takes its type back.
fn restore_types(value: &mut Value, typed: &BTreeMap<String, Value>) {
    match value {
        Value::String(text) => {
            if let Some(restored) = text
                .strip_prefix("${ENV.")
                .and_then(|rest| rest.strip_suffix('}'))
                .and_then(|key| typed.get(key))
            {
                *value = restored.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| restore_types(item, typed)),
        Value::Object(map) => map.values_mut().for_each(|item| restore_types(item, typed)),
        _ => {}
    }
}

/// Every placeholder key a definition holds.
pub(crate) fn placeholders_in(server: &McpServer) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    let mut value = serde_json::to_value(server).unwrap_or(Value::Null);
    each_string(&mut value, &mut |text| placeholder_keys(text, &mut keys));
    keys
}

/// Keys of `text` in placeholder form, for a label or an env key.
pub(crate) fn placeholders_of(text: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    placeholder_keys(text, &mut keys);
    keys
}

/// `text` with the owner's values in place of its placeholders.
pub(crate) fn fill_text(text: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = text.to_string();
    for (key, filled) in values {
        if placeholder_key(key) && !filled.is_empty() {
            out = out.replace(&format!("${{ENV.{key}}}"), filled);
        }
    }
    out
}

/// The definition with the owner's values in place of the placeholders, and
/// the keys left unfilled (they stay as references and fail a call naming
/// them). A typed placeholder goes back as its type when the value parses.
pub(crate) fn fill(
    server: &McpServer,
    values: &BTreeMap<String, String>,
    types: &BTreeMap<String, ExportValueType>,
) -> anyhow::Result<(McpServer, Vec<String>)> {
    let mut value = serde_json::to_value(server)?;
    let typed: BTreeMap<String, Value> = values
        .iter()
        .filter_map(|(key, text)| {
            let parsed = match types.get(key)? {
                ExportValueType::Text => return None,
                ExportValueType::Number => {
                    Value::Number(serde_json::from_str::<serde_json::Number>(text.trim()).ok()?)
                }
                ExportValueType::Boolean => Value::Bool(text.trim().parse::<bool>().ok()?),
            };
            Some((key.clone(), parsed))
        })
        .collect();
    restore_types(&mut value, &typed);
    each_string(&mut value, &mut |text| *text = fill_text(text, values));
    let filled: McpServer = serde_json::from_value(value)?;
    let left = placeholders_in(&filled).into_iter().collect();
    Ok((filled, left))
}

#[cfg(all(test, feature = "full-tests"))]
#[path = "redaction_tests.rs"]
pub(crate) mod tests;
