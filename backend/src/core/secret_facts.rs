//! KT-1404 — the secrets committed to the repository, as facts for the audit.
//!
//! The audit wrote "no hardcoded secrets detected" next to a literal token in
//! `.env.dist`, a Teams webhook URL in a Jenkinsfile and a committed private
//! key it dismissed as "a test key". The security prompt forbids the agent to
//! open a secret value (its context leaves the machine), so it had nothing to
//! base a verdict on and wrote the comfortable one. Kronn reads the tracked
//! files itself instead and hands over WHERE a secret is, never what it is.
//!
//! A value never leaves this module: the scan keeps a path and a [`Kind`] per
//! file, nothing of the matched text, so no prompt, document or log can carry
//! one. Every file is read through [`rooted_io`], bounded.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use crate::core::rooted_io;

/// The most one tracked file may weigh to be scanned.
const MAX_FILE: u64 = 256 * 1024;
/// The most files one scan opens: a pathological repository cannot stall a step.
const MAX_FILES: usize = 6_000;
/// The most bytes one scan reads in total.
const MAX_TOTAL: u64 = 96 * 1024 * 1024;
/// Paths listed in the prompt; the rest is counted.
const SHOWN: usize = 25;

/// What a tracked file holds, in decreasing order of gravity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A PEM private key body (`-----BEGIN … PRIVATE KEY-----` and its base64).
    PrivateKeyBlock,
    /// A file NAMED like a private key or a credential store (`id_rsa`, `*.pem`,
    /// a file beside its `.pub`, anything under `.ssh/`, `.htpasswd`) whose
    /// content did not prove it: a candidate, never a finding on its own.
    PrivateKeyFile,
    /// A well-known vendor token (GitHub, AWS, Slack, Stripe, GitLab, Atlassian).
    VendorToken,
    /// A Slack / Teams / Discord incoming-webhook URL: whoever holds it posts as you.
    WebhookUrl,
    /// A literal value assigned to a token / secret / password / API-key name.
    LiteralCredential,
    /// A tracked `.env` file that is not a template, by name only.
    EnvFile,
}

impl Kind {
    /// Whether the content of the file showed it, not only its name.
    fn proven(self) -> bool {
        !matches!(self, Kind::PrivateKeyFile | Kind::EnvFile)
    }

    fn label(self) -> &'static str {
        match self {
            Kind::PrivateKeyBlock => "private key",
            Kind::PrivateKeyFile => "named like a private-key or credential file",
            Kind::VendorToken => "vendor token",
            Kind::WebhookUrl => "webhook URL",
            Kind::LiteralCredential => {
                "literal credential assigned to a token/secret/password name"
            }
            Kind::EnvFile => "tracked `.env` file (not a template)",
        }
    }
}

/// One tracked path and what was found in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub path: String,
    /// Most serious kind first.
    pub kinds: Vec<Kind>,
    /// The path sits in a test or fixture location.
    pub test_path: bool,
}

/// How far a finding is established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// The content matched a secret shape, outside the test locations.
    Confirmed,
    /// The content matched, in a test or fixture location: to verify.
    Fixture,
    /// Only the name matched: a candidate to check.
    Candidate,
}

impl Finding {
    fn tier(&self) -> Tier {
        match (self.kinds.iter().any(|k| k.proven()), self.test_path) {
            (true, false) => Tier::Confirmed,
            (true, true) => Tier::Fixture,
            (false, _) => Tier::Candidate,
        }
    }
}

/// The secrets found in the tracked files, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretFacts {
    /// Tracked paths with a finding, most serious first.
    pub findings: Vec<Finding>,
    /// Tracked files whose content was read.
    pub scanned: usize,
    /// Tracked files left unread: binary, over the cap, unreadable, or past the scan budget.
    pub skipped: usize,
    /// Whether the tracked files could be listed at all (a git repository).
    pub listed: bool,
    /// The flagged values, to scrub them from facts copied out of other files.
    pub literals: Literals,
}

/// Extensions of files that are not text worth scanning (or are generated).
const SKIPPED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "svg", "pdf", "zip", "gz", "tgz", "bz2",
    "xz", "7z", "rar", "jar", "war", "class", "so", "dll", "exe", "dylib", "woff", "woff2", "ttf",
    "otf", "eot", "mp3", "mp4", "mov", "avi", "webm", "wasm", "lock", "map", "min", "psd",
    "sketch", "xlsx", "docx", "pptx", "sqlite", "db",
];

/// Directories whose content is somebody else's code or a build output.
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    ".yarn",
    ".pnpm-store",
    "target",
    "dist",
    "build",
    ".git",
];

fn skipped_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.split('/').any(|part| SKIPPED_DIRS.contains(&part)) {
        return true;
    }
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if name.ends_with(".min.js") || name.ends_with(".min.css") {
        return true;
    }
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| SKIPPED_EXTENSIONS.contains(&ext))
}

/// Whether the file is read for `key: value` credential assignments: config,
/// env, CI, build and shell files. Source code is not (a `$token = $request->…`
/// is not a literal), only for the shapes that identify themselves.
fn config_like(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name.starts_with(".env")
        || name.contains(".env.")
        || name.ends_with(".env")
        || name.starts_with("jenkins")
        || name.starts_with("dockerfile")
        || name.starts_with("docker-compose")
        || name == ".npmrc"
        || name == ".netrc"
        || name == ".pgpass"
    {
        return true;
    }
    matches!(
        name.rsplit_once('.').map(|(_, ext)| ext),
        Some(
            "dist"
                | "yml"
                | "yaml"
                | "ini"
                | "properties"
                | "conf"
                | "cfg"
                | "toml"
                | "sh"
                | "bash"
                | "groovy"
                | "gradle"
                | "tf"
                | "tfvars"
                | "json"
                | "env"
        )
    )
}

// ─── Names ───────────────────────────────────────────────────────────────

/// A tracked file whose NAME says it holds a private key or a credential store.
fn key_file_by_name(path: &str, tracked: &HashSet<&str>) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if name.ends_with(".pub") {
        return false;
    }
    if let Some((_, ext)) = name.rsplit_once('.') {
        if matches!(
            ext,
            "pem" | "key" | "p12" | "pfx" | "jks" | "keystore" | "ppk" | "p8"
        ) {
            return true;
        }
    }
    if ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"]
        .iter()
        .any(|stem| name.starts_with(stem))
    {
        return true;
    }
    if matches!(name, ".htpasswd" | ".netrc" | ".pgpass") {
        return true;
    }
    // Everything under a `.ssh/` directory, but the files that hold no secret.
    if lower.split('/').rev().skip(1).any(|part| part == ".ssh")
        && !matches!(name, "known_hosts" | "config" | "authorized_keys")
    {
        return true;
    }
    // `X` beside a tracked `X.pub` is the private half.
    tracked.contains(format!("{path}.pub").as_str())
}

// ─── Content ─────────────────────────────────────────────────────────────

struct Patterns {
    webhook: regex_lite::Regex,
    vendor: Vec<regex_lite::Regex>,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let compile = |source: &str| regex_lite::Regex::new(source).expect("a valid pattern");
        Patterns {
            webhook: compile(concat!(
                r"https://(?:hooks\.slack\.com/(?:services|workflows)/[A-Za-z0-9/_-]{20,}",
                r"|[A-Za-z0-9.-]+\.webhook\.office\.com/webhook[A-Za-z0-9]*/[A-Za-z0-9@/_-]{20,}",
                r"|outlook\.office(?:365)?\.com/webhook/[A-Za-z0-9@/_-]{20,}",
                r"|discord(?:app)?\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]{20,})",
            )),
            vendor: [
                r"\bgh[pousr]_[A-Za-z0-9]{36,}",
                r"\bAKIA[0-9A-Z]{16}\b",
                r"\bxox[baprs]-[A-Za-z0-9-]{20,}",
                r"\bsk_live_[A-Za-z0-9]{20,}",
                r"\bglpat-[A-Za-z0-9_-]{20,}",
                r"\bATATT[A-Za-z0-9_=-]{30,}",
                r"\bsk-[A-Za-z0-9_-]{32,}",
            ]
            .iter()
            .map(|source| compile(source))
            .collect(),
        }
    })
}

/// Cheap guard before the regexes: a prefix any vendor token starts with.
fn may_hold_vendor_token(text: &str) -> bool {
    [
        "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "AKIA", "xox", "sk_live_", "glpat-", "ATATT", "sk-",
    ]
    .iter()
    .any(|prefix| text.contains(prefix))
}

/// Words that make a value a placeholder, not a credential.
const PLACEHOLDER_WORDS: &[&str] = &[
    "example",
    "your",
    "change",
    "dummy",
    "fake",
    "sample",
    "xxx",
    "placeholder",
    "replace",
    "todo",
    "foobar",
    "secret",
    "password",
    "token",
    "insert",
    "redacted",
    "undefined",
    "localhost",
    "notasecret",
    "test",
];

fn placeholder(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if PLACEHOLDER_WORDS.iter().any(|word| lower.contains(word)) {
        return true;
    }
    // `aaaaaaaa`, `00000000`, `abcabcabc`: not entropy.
    let distinct: HashSet<char> = lower.chars().collect();
    distinct.len() <= 3 || has_run(&lower)
}

/// Eight characters in a row that repeat (`aaaaaaaa`) or count up
/// (`abcdefgh`, `12345678`): the alphabet and digit strings of a fixture, not a
/// random token.
fn has_run(lower: &str) -> bool {
    const RUN: usize = 8;
    let bytes = lower.as_bytes();
    let (mut same, mut up) = (1usize, 1usize);
    for pair in bytes.windows(2) {
        same = if pair[1] == pair[0] { same + 1 } else { 1 };
        up = if pair[1] == pair[0].wrapping_add(1) && pair[1].is_ascii_alphanumeric() {
            up + 1
        } else {
            1
        };
        if same >= RUN || up >= RUN {
            return true;
        }
    }
    false
}

/// A PEM private key header followed by a base64 body line.
fn has_private_key_block(text: &str) -> bool {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let head = line.trim();
        if !(head.starts_with("-----BEGIN ") && head.ends_with("PRIVATE KEY-----")) {
            continue;
        }
        for next in lines.by_ref().take(4) {
            let body = next.trim();
            if body.contains(':') {
                continue; // `Proc-Type:` / `DEK-Info:` headers of an encrypted key
            }
            if body.len() >= 16
                && body
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
            {
                return true;
            }
            break;
        }
    }
    false
}

/// The words of a name: `GLOBAL_API_TOKEN`, `clientSecret`, `//reg/:_authToken`.
fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            previous_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && previous_lower && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        previous_lower = c.is_ascii_lowercase();
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// A name that holds a credential, not something about one (`TOKEN_URL`,
/// `PASSWORD_FILE`, `token_ttl`).
fn credential_name(name: &str) -> bool {
    const SECRET: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "credential",
        "credentials",
    ];
    const ABOUT: &[&str] = &[
        "file",
        "path",
        "dir",
        "url",
        "uri",
        "name",
        "id",
        "ttl",
        "expiry",
        "expires",
        "expiration",
        "length",
        "header",
        "type",
        "field",
        "param",
        "endpoint",
        "regex",
        "pattern",
        "label",
        "required",
        "enabled",
        "disabled",
        "version",
        "algorithm",
        "alg",
        "min",
        "max",
        "size",
        "count",
        "policy",
        "provider",
        "manager",
        "service",
    ];
    let words = words(name);
    // A word about the credential AFTER its name makes the name metadata
    // (`TOKEN_URL`, `password_file`); one BEFORE qualifies it (`SERVICE_TOKEN`,
    // `PROVIDER_SECRET`) and the value is still a credential.
    let mut last = words
        .iter()
        .rposition(|w| SECRET.contains(&w.as_str()) || w == "apikey");
    for (at, pair) in words.windows(2).enumerate() {
        if matches!(
            (pair[0].as_str(), pair[1].as_str()),
            ("api", "key") | ("private", "key") | ("access", "key") | ("auth", "key")
        ) {
            last = last.max(Some(at + 1));
        }
    }
    let Some(last) = last else {
        return false;
    };
    !words[last + 1..]
        .iter()
        .any(|w| ABOUT.contains(&w.as_str()))
}

/// A value that is a literal: long enough, one token without spaces or
/// substitution syntax, with letters and digits, not a placeholder.
/// The literal a value is, cleaned of its quotes, trailing comma and comment.
fn literal_of(raw: &str) -> Option<&str> {
    let value = raw
        .split(" #")
        .next()
        .unwrap_or(raw)
        .trim()
        .trim_end_matches([',', ';'])
        .trim()
        .trim_matches(|c| c == '"' || c == '\'');
    if !(8..=512).contains(&value.len()) {
        return None;
    }
    if value.starts_with([
        '$', '%', '{', '<', '[', '(', '@', '*', '&', '!', '|', '>', '#',
    ]) {
        return None;
    }
    if ["${", "{{", "%env", "$(", "<?", "://", "::"]
        .iter()
        .any(|marker| value.contains(marker))
    {
        return None;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "+/=_.-~@!#$%^&*".contains(c))
    {
        return None;
    }
    let letters = value.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digits = value.chars().filter(|c| c.is_ascii_digit()).count();
    (letters >= 4 && digits >= 1 && !placeholder(value)).then_some(value)
}

/// The literal a line assigns to a credential name — `KEY=value`,
/// `export KEY=value`, `key: value`, `"key": "value",` — if it does.
fn credential_literal(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        // `// note` is a comment; `//registry.npmjs.org/:_authToken=…` is an npmrc entry.
        || trimmed.starts_with("// ")
        || trimmed == "//"
        || trimmed.starts_with(';')
    {
        return None;
    }
    let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed);
    let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
    // `KEY=value` and `key: value`; an npmrc line is both (`//host/:_authToken=v`).
    ['=', ':'].into_iter().find_map(|separator| {
        let at = trimmed.find(separator)?;
        let (key, value) = (&trimmed[..at], &trimmed[at + 1..]);
        let key = key.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let named = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':'))
            && credential_name(key);
        if named {
            literal_of(value)
        } else {
            None
        }
    })
}

/// Whether the line assigns a literal to a credential name.
fn assigns_literal_credential(line: &str) -> bool {
    credential_literal(line).is_some()
}

/// The values the scan flagged, kept in memory only so a fact copied from
/// another file can be scrubbed of them. Never printed: `Debug` shows a count,
/// and no renderer reads the values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Literals(Vec<String>);

impl std::fmt::Debug for Literals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Literals({})", self.0.len())
    }
}

/// The most distinct values kept.
const MAX_LITERALS: usize = 512;
/// A shorter value would redact common words.
const MIN_LITERAL: usize = 6;

impl Literals {
    fn add(&mut self, value: &str) {
        if value.len() >= MIN_LITERAL
            && self.0.len() < MAX_LITERALS
            && !self.0.iter().any(|known| known == value)
        {
            self.0.push(value.to_string());
        }
    }

    /// Records every value `scan_text` would flag in `text`.
    fn collect(&mut self, path: &str, text: &str) {
        if may_hold_vendor_token(text) {
            for re in &patterns().vendor {
                for hit in re.find_iter(text) {
                    if !placeholder(hit.as_str()) {
                        self.add(hit.as_str());
                    }
                }
            }
        }
        if text.contains("https://") {
            for hit in patterns().webhook.find_iter(text) {
                let tail = hit.as_str().rsplit('/').next().unwrap_or("");
                if !placeholder(tail) {
                    self.add(hit.as_str());
                    self.add(tail);
                }
            }
        }
        if config_like(path) {
            for line in text.lines() {
                if let Some(value) = credential_literal(line) {
                    self.add(value);
                }
            }
        }
    }

    /// `text` with every kept value replaced by a marker, longest first so a
    /// value that contains another is removed whole.
    pub fn scrub(&self, text: &str) -> String {
        let mut values: Vec<&String> = self.0.iter().collect();
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        let mut out = text.to_string();
        for value in values {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), "<redacted>");
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What one text file holds. Pure: nothing of the matched text is returned.
pub fn scan_text(path: &str, text: &str) -> Vec<Kind> {
    let mut kinds = Vec::new();
    if has_private_key_block(text) {
        kinds.push(Kind::PrivateKeyBlock);
    }
    if may_hold_vendor_token(text)
        && patterns()
            .vendor
            .iter()
            .any(|re| re.find_iter(text).any(|hit| !placeholder(hit.as_str())))
    {
        kinds.push(Kind::VendorToken);
    }
    if text.contains("https://") {
        let live = patterns().webhook.find_iter(text).any(|hit| {
            let tail = hit.as_str().rsplit('/').next().unwrap_or("");
            !placeholder(tail)
        });
        if live {
            kinds.push(Kind::WebhookUrl);
        }
    }
    if config_like(path) && text.lines().any(assigns_literal_credential) {
        kinds.push(Kind::LiteralCredential);
    }
    kinds
}

/// A text taken from a file, with every value the scan would flag replaced by a
/// marker: a webhook URL, a vendor token, a high-entropy token, and the value a
/// credential name is compared to or assigned (`TOKEN == 'v'`, `key: v`). What
/// is left says what the text does (the label, the event, the branch) and
/// carries no secret. This is the guard for any fact copied out of a repository
/// file into a prompt, a document or a log.
pub fn redact(text: &str) -> String {
    static SHAPES: OnceLock<(regex_lite::Regex, regex_lite::Regex)> = OnceLock::new();
    let (assignment, entropy) = SHAPES.get_or_init(|| {
        (
            regex_lite::Regex::new(
                r#"([A-Za-z0-9_.\-/]*[A-Za-z0-9_.\-]+)(\s*(?:==|!=|=|:)\s*)('[^']*'|"[^"]*"|[^\s'",;)}\]]+)"#,
            )
            .expect("a valid pattern"),
            regex_lite::Regex::new(r"[A-Za-z0-9+/=_~-]{20,}").expect("a valid pattern"),
        )
    });
    let text = redact_context_operands(text);
    let mut out = patterns()
        .webhook
        .replace_all(&text, "<webhook URL redacted>")
        .into_owned();
    for vendor in &patterns().vendor {
        out = vendor.replace_all(&out, "<token redacted>").into_owned();
    }
    // A value that follows a credential name, quoted or not; references
    // (`secrets.X`, `${{ … }}`, `$VAR`) and empty values name no secret.
    out = assignment
        .replace_all(&out, |caps: &regex_lite::Captures| {
            let value = caps[3].trim_matches(|c| c == '\'' || c == '"');
            let reference = value.is_empty()
                || value.starts_with(['$', '%', '<'])
                || value.starts_with("secrets.")
                || value.starts_with("env.")
                || value.starts_with("vars.")
                || value.starts_with("github.")
                || value.starts_with("inputs.")
                || matches!(value, "true" | "false" | "null");
            if !reference && value.len() >= 6 && credential_name(&caps[1]) {
                format!("{}{}<redacted>", &caps[1], &caps[2])
            } else {
                caps[0].to_string()
            }
        })
        .into_owned();
    entropy
        .replace_all(&out, |caps: &regex_lite::Captures| {
            let token = &caps[0];
            let has = |f: fn(&char) -> bool| token.chars().any(|c| f(&c));
            if has(char::is_ascii_uppercase)
                && has(char::is_ascii_lowercase)
                && has(char::is_ascii_digit)
                && !placeholder(token)
            {
                "<token redacted>".to_string()
            } else {
                token.to_string()
            }
        })
        .into_owned()
}

/// Every quoted literal that is an operand of a comparison (`==`, `!=`) or of
/// `contains()` / `startsWith()` / `endsWith()` whose other operand is an
/// `env.` / `secrets.` / `vars.` context, replaced by a marker whatever its
/// position (`env.X == 'v'`, `'v' == env.X`, `contains(env.X, 'v')`,
/// `contains('v', env.X)`). A literal tested against a context is what a
/// credential gate looks like, and its value may be one. Label and event names
/// (`github.event.label.name == 'ci-test'`) compare to no such context and stay.
fn redact_context_operands(text: &str) -> String {
    struct Shapes {
        /// `OP ==`, `OP !=` and `contains(OP,` right before the literal.
        before_operand: regex_lite::Regex,
        /// `contains(` right before the literal.
        before_call: regex_lite::Regex,
        /// `== OP` right after the literal.
        after_comparison: regex_lite::Regex,
        /// `, OP)` right after the literal.
        after_call: regex_lite::Regex,
    }
    static SHAPES: OnceLock<Shapes> = OnceLock::new();
    let shapes = SHAPES.get_or_init(|| {
        let operand = r#"(?:env|secrets|vars)(?:\.[A-Za-z0-9_\-]+|\[\s*['"][^'"]*['"]\s*\])"#;
        let call = r"(?:contains|startsWith|endsWith)\(\s*";
        let compile = |source: String| regex_lite::Regex::new(&source).expect("a valid pattern");
        Shapes {
            before_operand: compile(format!(
                r"(?:{operand}\s*(?:==|!=)\s*|{call}{operand}\s*,\s*)$"
            )),
            before_call: compile(format!(r"{call}$")),
            after_comparison: compile(format!(r"^\s*(?:==|!=)\s*{operand}")),
            after_call: compile(format!(r"^\s*,\s*{operand}\s*\)")),
        }
    });
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'\'' && quote != b'"' {
            i += 1;
            continue;
        }
        // A quoted literal runs to the next same quote (`''` inside is one quote).
        let mut end = i + 1;
        while end < bytes.len() {
            if bytes[end] == quote {
                if bytes.get(end + 1) == Some(&quote) {
                    end += 2;
                    continue;
                }
                break;
            }
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let (before, after) = (&text[..i], &text[end + 1..]);
        let operand = shapes.before_operand.is_match(before)
            || shapes.after_comparison.is_match(after)
            || (shapes.before_call.is_match(before) && shapes.after_call.is_match(after));
        if operand && end > i + 1 {
            out.push_str(&text[copied..=i]);
            out.push_str("<redacted>");
            copied = end;
        }
        i = end + 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// A path that may be written in a prompt: no control character, no markup.
fn printable(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 200
        && path
            .chars()
            .all(|c| !c.is_control() && !matches!(c, '`' | '<' | '>' | '[' | ']'))
}

impl SecretFacts {
    /// Scans the tracked files of `project`; `tracked` is `git ls-files`, `None`
    /// when the project is not a git repository.
    pub fn detect(project: &Path, tracked: Option<&[String]>) -> Self {
        let Some(tracked) = tracked else {
            return Self::default();
        };
        let names: HashSet<&str> = tracked.iter().map(String::as_str).collect();
        let mut facts = Self {
            listed: true,
            ..Self::default()
        };
        let mut found: Vec<Finding> = Vec::new();
        let mut read_total = 0u64;
        for path in tracked {
            let mut kinds = Vec::new();
            if crate::core::audit_baseline::is_tracked_env_file(path) {
                kinds.push(Kind::EnvFile);
            }
            let mut named_key = key_file_by_name(path, &names) && !skipped_dir_only(path);
            let mut in_test_region = false;
            if !skipped_path(path) {
                if facts.scanned >= MAX_FILES || read_total >= MAX_TOTAL {
                    facts.skipped += 1;
                } else {
                    match rooted_io::read_capped(project, Path::new(path), MAX_FILE) {
                        Ok(bytes) if !bytes.contains(&0) => {
                            read_total += bytes.len() as u64;
                            facts.scanned += 1;
                            if let Ok(text) = std::str::from_utf8(&bytes) {
                                facts.literals.collect(path, text);
                                let found = scan_text(path, text);
                                // Rust keeps its tests at the end of the file: a hit
                                // that only the part after `#[cfg(test)]` has is a fixture.
                                if !found.is_empty()
                                    && path.ends_with(".rs")
                                    && rust_test_start(text).is_some_and(|start| {
                                        scan_text(path, &text[..start]).is_empty()
                                    })
                                {
                                    in_test_region = true;
                                }
                                kinds.extend(found);
                                // A `.pem` that holds only certificates or public
                                // keys is public material: its name proves nothing.
                                if named_key && only_public_material(text) {
                                    named_key = false;
                                }
                            }
                        }
                        _ => facts.skipped += 1,
                    }
                }
            }
            if named_key {
                kinds.push(Kind::PrivateKeyFile);
            }
            kinds.sort();
            kinds.dedup();
            if !kinds.is_empty() {
                found.push(Finding {
                    path: path.clone(),
                    kinds,
                    test_path: in_test_region || test_location(path),
                });
            }
        }
        found.sort_by(|a, b| {
            (a.tier() as u8)
                .cmp(&(b.tier() as u8))
                .then_with(|| a.kinds[0].cmp(&b.kinds[0]))
                .then_with(|| a.path.cmp(&b.path))
        });
        facts.findings = found;
        facts
    }

    /// The prompt block for the steps that judge security or state it.
    pub fn render_block(&self) -> String {
        let mut out = String::from(
            "## Secret facts — computed by Kronn from the tracked files (ground truth: paths and kinds only, never a value)\n\n",
        );
        if !self.listed {
            out.push_str(
                "The tracked files could not be listed (not a git repository): Kronn did not evaluate committed secrets. For every secret item write `Not evaluated safely`; never write that there are no secrets.\n",
            );
            return out;
        }
        out.push_str(&format!(
            "Kronn read {} tracked text file(s) ({} left unread: binary, over {} KiB, unreadable or past the scan budget) for: PEM private-key bodies, webhook URLs (Slack, Teams, Discord), well-known vendor tokens and literal values assigned to token/secret/password/API-key names in config, env, CI and shell files; it also lists files NAMED like a key store. It keeps no value.\n",
            self.scanned,
            self.skipped,
            MAX_FILE / 1024
        ));
        let tier = |wanted: Tier| -> Vec<&Finding> {
            self.findings
                .iter()
                .filter(|f| f.tier() == wanted)
                .collect()
        };
        let (confirmed, fixtures, candidates) = (
            tier(Tier::Confirmed),
            tier(Tier::Fixture),
            tier(Tier::Candidate),
        );
        if self.findings.is_empty() {
            out.push_str("Detected: none.\n");
        }
        let list = |out: &mut String, title: &str, items: &[&Finding]| {
            if items.is_empty() {
                return;
            }
            out.push_str(&format!("{title} ({}):\n", items.len()));
            for finding in items.iter().take(SHOWN) {
                let shown = if printable(&finding.path) {
                    format!("`{}`", finding.path)
                } else {
                    "a path with unprintable characters".to_string()
                };
                let kinds: Vec<&str> = finding.kinds.iter().map(|k| k.label()).collect();
                out.push_str(&format!("- {shown} — {}\n", kinds.join("; ")));
            }
            if items.len() > SHOWN {
                out.push_str(&format!(
                    "- … {} more path(s) not shown\n",
                    items.len() - SHOWN
                ));
            }
        };
        list(
            &mut out,
            "Detected by CONTENT in tracked paths (the content matched a secret shape)",
            &confirmed,
        );
        list(
            &mut out,
            "Detected by content in TEST or FIXTURE paths (to verify)",
            &fixtures,
        );
        list(
            &mut out,
            "CANDIDATES matched by NAME only (the content did not prove a secret: a template, an opaque or binary file)",
            &candidates,
        );
        out.push_str(
            "\nRules for every secret item (the secrets checklist, the Security dimension, the final review):\n",
        );
        let mut rule = 0;
        let mut push_rule = |out: &mut String, text: &str| {
            rule += 1;
            out.push_str(&format!("{rule}. {text}\n"));
        };
        if !confirmed.is_empty() {
            push_rule(
                &mut out,
                "Each path listed as detected by CONTENT is a committed secret: report one finding per path (Critical or High), cited `[src: file: <path>:exists]`, with the remediation \"revoke it at the provider, then purge it from the git history\". Never open the file for its value, never quote or print any value, in any file.",
            );
            push_rule(
                &mut out,
                "A committed private key, token or webhook URL is a defect whatever its name says: `test`, `example`, `dev`, `ci`, a hostname or a template suffix does not make it harmless, and it is never dismissed as \"a test key\" or \"not a real secret\". Only a repository document that states the key was revoked can lower the severity, and you cite it.",
            );
        }
        if !fixtures.is_empty() {
            push_rule(
                &mut out,
                "A path in a test or fixture location whose content matched is `to verify`, Medium at most, cited `[src: file: <path>:exists]`: say Kronn matched a secret shape there, never that it is real and never that it is harmless without a repository document saying so.",
            );
        }
        if !candidates.is_empty() {
            push_rule(
                &mut out,
                "A CANDIDATE is a file whose NAME looks like a key or a credential store; Kronn could not prove a secret from its content (a public certificate, a template and an opaque or binary store all match). Report it as `to verify` (Low or Medium), worded as a candidate, never as a confirmed secret and never Critical or High; do not open the file for its value.",
            );
        }
        if confirmed.is_empty() && fixtures.is_empty() {
            push_rule(
                &mut out,
                &if candidates.is_empty() {
                    "Kronn found none: write `none detected by Kronn's scan`, never \"no secrets\" or \"verified absent\", and say what the scan cannot see (the git history, encoded or split values, files it left unread, formats it does not know).".to_string()
                } else {
                    "Kronn confirmed no secret from content: write `no secret confirmed by Kronn's scan; N candidate(s) to verify`, never \"no secrets\" or \"verified absent\", and say what the scan cannot see (the git history, encoded or split values, files it left unread, formats it does not know).".to_string()
                },
            );
        } else {
            push_rule(
                &mut out,
                "Never write \"no secrets\", \"no hardcoded secrets\" or \"verified absent\" for secrets anywhere in your output while a path above is detected by content, and never close or downgrade a secret finding on a word.",
            );
        }
        out
    }
}

/// Where the test part of a Rust file starts: its first `#[cfg(test)]` (or
/// `#[cfg(all(test, …))]`) attribute.
fn rust_test_start(text: &str) -> Option<usize> {
    ["#[cfg(test)]", "#[cfg(all(test"]
        .iter()
        .filter_map(|marker| text.find(marker))
        .min()
}

/// A PEM file that holds certificates or public keys and no private key.
fn only_public_material(text: &str) -> bool {
    let public = text.contains("-----BEGIN CERTIFICATE-----")
        || text.contains("-----BEGIN TRUSTED CERTIFICATE-----")
        || text.contains("-----BEGIN X509 CRL-----")
        || text.contains("-----BEGIN CERTIFICATE REQUEST-----")
        || text.contains("PUBLIC KEY-----");
    public && !text.contains("PRIVATE KEY-----")
}

/// Whether the path is a test or fixture location, where a credential is more
/// often a test double than a leak.
fn test_location(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    if parts.iter().any(|part| {
        matches!(
            *part,
            "tests"
                | "test"
                | "__tests__"
                | "spec"
                | "specs"
                | "fixtures"
                | "__fixtures__"
                | "testdata"
                | "e2e"
                | "mocks"
                | "__mocks__"
        )
    }) {
        return true;
    }
    // Test SOURCE files only: a file named `test_key` is not softened by its name.
    name.contains(".test.")
        || name.ends_with(".test")
        || name.contains(".spec.")
        || [
            "_test.go",
            "_test.py",
            "_test.rs",
            "_tests.rs",
            "test.php",
            "test.java",
            "tests.java",
            "test.kt",
            "tests.kt",
            "_spec.rb",
            "_test.rb",
            "test.cs",
            "tests.cs",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// A key file under a directory that is somebody else's code is not the
/// repository's own (`vendor/…/id_rsa` fixtures of a dependency).
fn skipped_dir_only(path: &str) -> bool {
    path.split('/')
        .any(|part| matches!(part, "node_modules" | "vendor" | ".yarn" | ".pnpm-store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// Built apart so no literal secret sits in this source file.
    fn token() -> String {
        ["a1B2c3D4", "e5F6g7H8", "i9J0k1L2"].concat()
    }

    fn webhook() -> String {
        format!(
            "https://{}.webhook.office.com/webhookb2/{}-{}/IncomingWebhook/{}",
            "contoso",
            "11111111-2222-3333-4444-555555555555",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "q7Zk2mX9vB4nR8tY1wC5eH3jL6pD0sFg",
        )
    }

    fn key_body() -> String {
        format!(
            "-----BEGIN {} KEY-----\n{}\n-----END {} KEY-----\n",
            "OPENSSH PRIVATE", "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ", "OPENSSH PRIVATE",
        )
    }

    fn tracked(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn a_literal_token_in_an_env_template_is_found_by_path_and_never_quoted() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = token();
        write(
            tmp.path(),
            ".env.dist",
            &format!("APP_ENV=dev\nGLOBAL_API_TOKEN={secret}\n"),
        );
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&[".env.dist"])));
        assert_eq!(
            facts.findings,
            vec![Finding {
                path: ".env.dist".to_string(),
                kinds: vec![Kind::LiteralCredential],
                test_path: false,
            }]
        );
        let block = facts.render_block();
        assert!(block.contains("`.env.dist`"), "{block}");
        assert!(!block.contains(&secret), "{block}");
        assert!(
            block.contains("never write \"no secrets\"")
                || block.contains("Never write \"no secrets\"")
        );
    }

    #[test]
    fn a_webhook_url_in_a_jenkinsfile_is_found_and_not_printed() {
        let tmp = tempfile::tempdir().unwrap();
        let url = webhook();
        write(
            tmp.path(),
            "Jenkinsfile.staging",
            &format!("pipeline {{\n  environment {{ HOOK = '{url}' }}\n}}\n"),
        );
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&["Jenkinsfile.staging"])));
        assert_eq!(facts.findings.len(), 1, "{facts:?}");
        assert!(facts.findings[0].kinds.contains(&Kind::WebhookUrl));
        let block = facts.render_block();
        assert!(!block.contains("webhookb2"), "{block}");
        assert!(!block.contains("q7Zk2mX9"), "{block}");
    }

    #[test]
    fn a_committed_private_key_is_a_defect_whatever_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".ssh/deploy-key", &key_body());
        write(tmp.path(), ".ssh/deploy-key.pub", "ssh-ed25519 AAAA host\n");
        write(tmp.path(), "keys/test_key", &key_body());
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&[
                ".ssh/deploy-key",
                ".ssh/deploy-key.pub",
                "keys/test_key",
            ])),
        );
        let paths: Vec<&str> = facts.findings.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec![".ssh/deploy-key", "keys/test_key"], "{facts:?}");
        assert!(facts.findings[0].kinds.contains(&Kind::PrivateKeyBlock));
        // `keys/test_key` is named like a test, not located in one.
        assert!(facts.findings.iter().all(|f| f.tier() == Tier::Confirmed));
        let block = facts.render_block();
        assert!(
            block.contains("never dismissed as \"a test key\""),
            "{block}"
        );
        assert!(
            !block.contains("b3BlbnNzaC1rZXktdjEAAAAABG5vbmU"),
            "{block}"
        );
        assert!(block.contains("whatever its name says"), "{block}");
    }

    #[test]
    fn a_key_with_no_content_marker_is_a_candidate_by_name_never_a_mandatory_finding() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "deploy/id_rsa", "opaque\n");
        write(tmp.path(), "certs/server.pem", "opaque\n");
        write(tmp.path(), "certs/server.pub", "opaque\n");
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&[
                "deploy/id_rsa",
                "certs/server.pem",
                "certs/server.pub",
            ])),
        );
        assert_eq!(facts.findings.len(), 2, "{facts:?}");
        assert!(facts
            .findings
            .iter()
            .all(|f| f.kinds == vec![Kind::PrivateKeyFile] && f.tier() == Tier::Candidate));
        let block = facts.render_block();
        assert!(block.contains("CANDIDATES matched by NAME only"), "{block}");
        assert!(block.contains("never as a confirmed secret"), "{block}");
        assert!(!block.contains("is a committed secret"), "{block}");
        assert!(!block.contains("whatever its name says"), "{block}");
        // Candidates alone never read as "no secrets" either.
        assert!(
            block.contains("no secret confirmed by Kronn's scan"),
            "{block}"
        );
    }

    #[test]
    fn a_public_certificate_is_not_a_secret_and_a_key_in_a_pem_still_is() {
        let tmp = tempfile::tempdir().unwrap();
        let cert = format!(
            "-----BEGIN {}-----\n{}\n-----END {}-----\n",
            "CERTIFICATE",
            "MIIDdzCCAl+gAwIBAgIEAgAAuTANBgkqhkiG9w0BAQUFADBaMQswCQYDVQQGEwJJ",
            "CERTIFICATE"
        );
        write(tmp.path(), "certs/ca.pem", &cert);
        write(
            tmp.path(),
            "certs/bundle.pem",
            &format!("{cert}{}", key_body()),
        );
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&["certs/ca.pem", "certs/bundle.pem"])),
        );
        let paths: Vec<&str> = facts.findings.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["certs/bundle.pem"], "{facts:?}");
        assert_eq!(facts.findings[0].tier(), Tier::Confirmed);
        let block = facts.render_block();
        assert!(!block.contains("ca.pem"), "{block}");
    }

    #[test]
    fn a_secret_in_a_test_or_fixture_path_is_to_verify_not_mandatory() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "tests/fixtures/id_key", &key_body());
        write(
            tmp.path(),
            "src/app.test.yml",
            &format!("API_TOKEN={}\n", token()),
        );
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&["tests/fixtures/id_key", "src/app.test.yml"])),
        );
        assert_eq!(facts.findings.len(), 2, "{facts:?}");
        assert!(facts.findings.iter().all(|f| f.tier() == Tier::Fixture));
        let block = facts.render_block();
        assert!(
            block.contains("TEST or FIXTURE paths (to verify)"),
            "{block}"
        );
        assert!(block.contains("Medium at most"), "{block}");
        assert!(
            !block.contains("Each path listed as detected by CONTENT"),
            "{block}"
        );
        // Still never "no secrets".
        assert!(block.contains("Never write \"no secrets\""), "{block}");
    }

    #[test]
    fn a_service_token_is_found_like_any_other_token_and_metadata_is_not() {
        let secret = token();
        for name in [
            "SERVICE_TOKEN",
            "GLOBAL_API_TOKEN",
            "PROVIDER_SECRET",
            "MANAGER_PASSWORD",
            "SERVICE_ACCOUNT_API_KEY",
        ] {
            assert!(
                assigns_literal_credential(&format!("{name}={secret}")),
                "{name} holds a credential"
            );
        }
        for name in [
            "TOKEN_SERVICE_URL",
            "TOKEN_URL",
            "PASSWORD_MANAGER_FILE",
            "API_KEY_PATH",
            "SECRET_TTL",
        ] {
            assert!(
                !assigns_literal_credential(&format!("{name}={secret}")),
                "{name} is about a credential"
            );
        }
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            ".env.dist",
            &format!("SERVICE_TOKEN={secret}\n"),
        );
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&[".env.dist"])));
        assert_eq!(facts.findings.len(), 1, "{facts:?}");
        assert!(!facts.render_block().contains("Detected: none"));
    }

    #[test]
    fn a_short_literal_token_of_eight_characters_is_found() {
        // A short `.env.dist` token is 8 characters: seven letters, one digit.
        assert!(assigns_literal_credential("GLOBAL_API_TOKEN=Qxzkmwb7"));
        assert!(!assigns_literal_credential("GLOBAL_API_TOKEN=Qxzkmwb"));
    }

    #[test]
    fn a_token_shape_in_the_test_part_of_a_rust_file_is_a_fixture_to_verify() {
        let tmp = tempfile::tempdir().unwrap();
        let shape = format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        let code = format!(
            "fn real() {{}}\n#[cfg(test)]\nmod tests {{\n    const T: &str = \"{shape}\";\n}}\n"
        );
        write(tmp.path(), "src/redact.rs", &code);
        let live = format!("const K: &str = \"{shape}\";\n#[cfg(test)]\nmod tests {{}}\n");
        write(tmp.path(), "src/live.rs", &live);
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&["src/redact.rs", "src/live.rs"])),
        );
        let tier = |path: &str| {
            facts
                .findings
                .iter()
                .find(|f| f.path == path)
                .map(Finding::tier)
        };
        assert_eq!(tier("src/redact.rs"), Some(Tier::Fixture), "{facts:?}");
        assert_eq!(tier("src/live.rs"), Some(Tier::Confirmed), "{facts:?}");
    }

    #[test]
    fn redact_keeps_the_gate_and_drops_the_value() {
        let secret = token();
        let long = format!("{}{}", "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8");
        let hook = webhook();
        let text = format!(
            "env.API_TOKEN == '{secret}' && contains(github.event.pull_request.labels.*.name, 'ci-test') && github.actor == '{secret}' && x == '{long}' && url == '{hook}'"
        );
        let out = redact(&text);
        for leaked in [secret.as_str(), "A1b2C3d4E5f6G7h8", "q7Zk2mX9", "webhookb2"] {
            assert!(!out.contains(leaked), "{out}");
        }
        assert!(out.contains("'ci-test'"), "{out}");
        assert!(
            out.contains("github.event.pull_request.labels.*.name"),
            "{out}"
        );
        assert!(out.contains("env.API_TOKEN =="), "{out}");
        // References and ordinary text pass through untouched.
        let plain = "github.event.label.name == 'deploy-prod-eu' && secrets.NPM_TOKEN != '' && github.ref == 'refs/heads/main'";
        assert_eq!(redact(plain), plain);
    }

    #[test]
    fn redact_drops_a_literal_compared_to_a_context_in_every_form() {
        // Short, so no entropy or vendor shape could catch it: only the form does.
        let marker = ["q7Zk", "2mX9"].concat();
        let forms = [
            format!("env.DEPLOY_TOKEN == '{marker}'"),
            format!("'{marker}' == env.DEPLOY_TOKEN"),
            format!("contains(env.DEPLOY_TOKEN, '{marker}')"),
            format!("contains('{marker}', env.DEPLOY_TOKEN)"),
            format!("startsWith(secrets.DEPLOY, \"{marker}\")"),
            format!("endsWith(vars.SUFFIX, '{marker}')"),
            format!("env.NAME != '{marker}'"),
            format!("vars.X == 'a' && 'b' == 'c' && '{marker}' == secrets.Y"),
            format!("env['DEPLOY'] == '{marker}'"),
        ];
        for form in forms {
            let out = redact(&form);
            assert!(!out.contains(&marker), "{out}");
            assert!(out.contains("<redacted>"), "{out}");
        }
        // A label or an event compared to no context stays readable.
        let gate = "contains(github.event.pull_request.labels.*.name, 'ci-test') && contains(env.NODE, '20') && github.event_name == 'pull_request'";
        let out = redact(gate);
        assert!(out.contains("'ci-test'"), "{out}");
        assert!(out.contains("'pull_request'"), "{out}");
        assert!(!out.contains("'20'"), "{out}");
    }

    #[test]
    fn the_flagged_values_scrub_any_text_and_are_never_printed() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = token();
        write(
            tmp.path(),
            ".env.dist",
            &format!("DEPLOY_TOKEN={secret}\nMODE=dev\n"),
        );
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&[".env.dist"])));
        assert!(!facts.literals.is_empty(), "{facts:?}");
        let text = format!("contains(foo.bar, '{secret}') && label == 'ci-test'");
        let out = facts.literals.scrub(&text);
        assert!(!out.contains(&secret), "{out}");
        assert!(out.contains("'ci-test'"), "{out}");
        assert!(!format!("{facts:?}").contains(&secret));
        assert!(!facts.render_block().contains(&secret));
    }

    #[test]
    fn placeholders_references_and_code_are_not_secrets() {
        for line in [
            "APP_SECRET=changeme-changeme-1",
            "API_TOKEN=",
            "API_TOKEN=${API_TOKEN}",
            "DB_PASSWORD=%env(resolve:DB_PASSWORD)%",
            "GLOBAL_TOKEN=<your token here>",
            "password: secret",
            "TOKEN_URL=https://example.org/oauth/token123",
            "PASSWORD_FILE=/run/secrets/db_password_1",
            "token_ttl: 3600000000000",
            "token: ${{ secrets.NPM_TOKEN }}",
            "SECRET_KEY_BASE=xxxxxxxxxxxxxxxxxxxxxxxx1",
            "# GLOBAL_API_TOKEN=a1B2c3D4e5F6g7H8",
            "name: build the token cache",
        ] {
            assert!(!assigns_literal_credential(line), "{line}");
        }
        let secret = token();
        for line in [
            format!("GLOBAL_API_TOKEN={secret}"),
            format!("export API_KEY=\"{secret}\""),
            format!("  clientSecret: {secret}"),
            format!("\"apiKey\": \"{secret}\","),
            format!("//registry.npmjs.org/:_authToken={secret}"),
        ] {
            assert!(assigns_literal_credential(&line), "{line}");
        }
    }

    #[test]
    fn source_code_that_mentions_the_formats_is_not_a_secret() {
        // A regex source, a test string and a doc placeholder are not secrets.
        let text = "let re = r\"-----BEGIN [A-Z ]*PRIVATE KEY-----\";\n\
                    let fixture = \"-----BEGIN OPENSSH PRIVATE KEY-----\\nb3Blbn\";\n\
                    // https://hooks.slack.com/services/T000000/B000000/XXXXXXXXXXXXXXXXXXXXXXXX\n";
        assert_eq!(scan_text("src/filter.rs", text), Vec::<Kind>::new());
        // `$token = $request->get('token')` is code, not a literal.
        assert_eq!(
            scan_text("src/Auth.php", "$token = 'a1B2c3D4e5F6g7H8i9J0';\n"),
            Vec::<Kind>::new()
        );
    }

    #[test]
    fn the_alphabet_strings_of_a_fixture_are_not_tokens() {
        // Redaction tests spell tokens as `abcdefghij…0123456789`: no entropy.
        let alphabet = format!("{}{}", "abcdefghijklmnopqrstuvwxyz", "0123456789");
        for text in [
            format!("let t = \"ghp_{alphabet}\";"),
            format!("let t = \"sk-proj-{alphabet}\";"),
            "let t = \"AKIAIOSFODNN7EXAMPLE\";".to_string(),
            format!("let t = \"xoxb-1234567890-{alphabet}\";"),
        ] {
            assert_eq!(
                scan_text("src/redact_test.rs", &text),
                Vec::<Kind>::new(),
                "{text}"
            );
        }
    }

    #[test]
    fn a_vendor_token_in_source_is_found() {
        let text = format!(
            "const k = '{}{}';\n",
            "ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"
        );
        assert_eq!(scan_text("src/a.js", &text), vec![Kind::VendorToken]);
    }

    #[test]
    fn a_real_dotenv_is_flagged_by_name_and_a_template_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".env", "APP_ENV=prod\n");
        write(tmp.path(), ".env.dist", "APP_ENV=dev\n");
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&[".env", ".env.dist"])));
        assert_eq!(
            facts.findings,
            vec![Finding {
                path: ".env".to_string(),
                kinds: vec![Kind::EnvFile],
                test_path: false,
            }]
        );
    }

    #[test]
    fn no_finding_never_reads_as_no_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "src/main.rs", "fn main() {}\n");
        let facts = SecretFacts::detect(tmp.path(), Some(&tracked(&["src/main.rs"])));
        assert!(facts.findings.is_empty());
        let block = facts.render_block();
        assert!(block.contains("Detected: none."), "{block}");
        assert!(block.contains("none detected by Kronn's scan"), "{block}");
        assert!(block.contains("never \"no secrets\""), "{block}");
    }

    #[test]
    fn without_a_git_listing_the_secrets_are_not_evaluated() {
        let facts = SecretFacts::detect(Path::new("."), None);
        let block = facts.render_block();
        assert!(block.contains("Not evaluated safely"), "{block}");
        assert!(
            block.contains("never write that there are no secrets"),
            "{block}"
        );
    }

    #[test]
    fn vendored_code_binaries_and_big_files_are_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let secret = token();
        write(
            tmp.path(),
            "vendor/x/.env.dist",
            &format!("A_TOKEN={secret}\n"),
        );
        write(tmp.path(), "logo.png", &format!("A_TOKEN={secret}\n"));
        write(
            tmp.path(),
            "big.yml",
            &format!("A_TOKEN={secret}\n{}", "x".repeat(300 * 1024)),
        );
        let facts = SecretFacts::detect(
            tmp.path(),
            Some(&tracked(&["vendor/x/.env.dist", "logo.png", "big.yml"])),
        );
        assert!(facts.findings.is_empty(), "{facts:?}");
        assert_eq!(facts.skipped, 1, "{facts:?}");
    }

    #[test]
    fn a_link_out_of_the_project_is_never_read() {
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let secret = token();
            write(outside.path(), "stolen.env", &format!("A_TOKEN={secret}\n"));
            std::os::unix::fs::symlink(
                outside.path().join("stolen.env"),
                project.path().join("linked.env.dist"),
            )
            .unwrap();
            let facts = SecretFacts::detect(project.path(), Some(&tracked(&["linked.env.dist"])));
            assert!(facts.findings.is_empty(), "{facts:?}");
        }
    }

    #[test]
    fn an_unprintable_path_is_not_written_in_the_prompt() {
        let facts = SecretFacts {
            findings: vec![Finding {
                path: "a`b\nc.pem".to_string(),
                kinds: vec![Kind::LiteralCredential],
                test_path: false,
            }],
            scanned: 1,
            skipped: 0,
            listed: true,
            ..SecretFacts::default()
        };
        let block = facts.render_block();
        assert!(!block.contains("a`b"), "{block}");
        assert!(block.contains("unprintable"), "{block}");
    }
}
