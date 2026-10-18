//! What an audit run can prove it read (KT-1308): the dependency watch it was
//! handed and the URLs its agent fetched. The documentary check holds the
//! `[src: deps: …]` and `[src: url: …]` citations of a written doc against
//! them whatever the anti-hallucination mode: a version "read" from a source
//! the run never opened is invented, and is refused in favour of
//! `not verified: no source`.

use std::collections::BTreeSet;

use crate::core::anti_halluc::{extract_source_markers, url_reference};
use crate::core::version_sources::{verify_dep_marker, DependencySnapshot};

/// A tool input is a few hundred bytes; a bound keeps a runaway stream cheap.
const MAX_TOOL_INPUT_BYTES: usize = 16 * 1024;

/// The citation check of one audit run.
#[derive(Debug, Clone, Default)]
pub struct RunProvenance {
    /// The dependency watch injected into the run; `None` when it had none, in
    /// which case no `deps:` citation can be backed.
    pub deps: Option<DependencySnapshot>,
    /// The URLs the agent fetched, normalized; `None` when the run's agent does
    /// not report its tool inputs (the fetches cannot be told apart, so a
    /// `url:` citation is left unchecked rather than refused).
    pub fetched: Option<BTreeSet<String>>,
}

impl RunProvenance {
    /// What the run holds so far: its dependency watch and the pages its agent
    /// fetched up to now.
    pub fn of(deps: &Option<DependencySnapshot>, tracker: &FetchTracker) -> Self {
        Self {
            deps: deps.clone(),
            fetched: tracker.fetched(),
        }
    }

    /// One message per `deps:` / `url:` citation of `content` the run cannot
    /// back. Quoted examples (fences, inline code) are not citations.
    pub fn problems(&self, content: &str) -> Vec<String> {
        let mut out = Vec::new();
        for raw in extract_source_markers(content) {
            if let Some((status, detail)) = verify_dep_marker(&raw, self.deps.as_ref()) {
                if status.is_fabricated() {
                    out.push(format!(
                        "`[src: {raw}]` is not backed by this run: {detail}. Write \
                         `not verified: no source` instead of a version the dependency watch \
                         does not hold"
                    ));
                }
                continue;
            }
            let (Some(fetched), Some(url)) = (&self.fetched, url_reference(&raw)) else {
                continue;
            };
            if !fetched.contains(&normalize_url(&url)) {
                out.push(format!(
                    "`[src: {raw}]` was never fetched in this run. Fetch that page and read it, \
                     or write `not verified: no source` instead of a version or date taken from \
                     memory"
                ));
            }
        }
        out
    }
}

/// The one safe, canonical form of a page address, used on the stored side
/// (what a run fetched) and on the cited side (what a doc cites): scheme,
/// userinfo, host case, a leading `www.`, fragment and trailing `/` removed,
/// the value of a credential-style query parameter masked as `name=***` (the
/// name is judged after percent-decoding, so `%73ig` is `sig`), and vendor
/// token shapes redacted. Non-secret parameters are kept as written. Two
/// spellings of one page compare equal, nothing stored holds a secret, and the
/// form is idempotent: a stored URL normalizes to itself.
pub fn normalize_url(url: &str) -> String {
    let url = url.trim().split('#').next().unwrap_or("");
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &url[scheme.len()..]))
        .unwrap_or(url);
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let (path, query) = match tail.split_once('?') {
        Some((path, query)) => (path, Some(mask_credential_params(query))),
        None => (tail, None),
    };
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let mut out = host.to_owned();
    if !path.is_empty() {
        out.push('/');
        out.push_str(path);
    }
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        out.push('?');
        out.push_str(&query);
    }
    crate::core::redact::redact_secrets(&out)
}

/// A query string whose credential-style values (signatures, tokens, keys,
/// passwords) are replaced by `***`: `sig`, `X-Amz-Signature`, `access_token`…
/// A parameter is read after percent-decoding, so an encoded name or an
/// encoded `=` hides nothing; the masked parameter is written decoded.
fn mask_credential_params(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    let mut start = 0;
    for (index, ch) in query.char_indices() {
        if ch == '&' || ch == ';' {
            out.push_str(&mask_param(&query[start..index]));
            out.push(ch);
            start = index + 1;
        }
    }
    out.push_str(&mask_param(&query[start..]));
    out
}

fn mask_param(pair: &str) -> String {
    let decoded = percent_decode(pair);
    match decoded.split_once('=') {
        Some((key, _)) if is_credential_param(key) => format!("{}=***", key.trim()),
        _ => pair.to_owned(),
    }
}

/// `%XX` and `+` decoded, repeatedly (`%2573` is `%73` is `s`): a name hidden
/// behind two layers of encoding is still read.
fn percent_decode(text: &str) -> String {
    let mut current = text.to_owned();
    for _ in 0..3 {
        let bytes = current.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
            match bytes[index] {
                b'%' if hex(index + 1).is_some() && hex(index + 2).is_some() => {
                    let high = hex(index + 1).unwrap_or(0);
                    let low = hex(index + 2).unwrap_or(0);
                    out.push((high * 16 + low) as u8);
                    index += 3;
                }
                b'+' => {
                    out.push(b' ');
                    index += 1;
                }
                byte => {
                    out.push(byte);
                    index += 1;
                }
            }
        }
        let next = String::from_utf8_lossy(&out).into_owned();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn is_credential_param(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['_', '-', ' '], "");
    matches!(key.as_str(), "sig" | "key" | "code" | "sas" | "auth")
        || [
            "signature",
            "token",
            "secret",
            "password",
            "passwd",
            "apikey",
            "credential",
            "authorization",
        ]
        .iter()
        .any(|word| key.contains(word))
}

/// Whether a tool, by the name its runtime reports, opens a page by the `url`
/// it is given: Claude's `WebFetch`, Kronn's `web_fetch`, an MCP fetch tool, a
/// browser's `browser_navigate` / `navigate_page` / `new_page`.
pub(crate) fn opens_url(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "fetch", "navigate", "new_page", "browse", "goto", "scrape", "crawl",
    ]
    .iter()
    .any(|word| name.contains(word))
}

/// Whether a tool is a documentation lookup whose RESULT names the pages it
/// read (Context7): the agent then cites a URL it was handed, not one it sent.
pub(crate) fn returns_urls(name: &str) -> bool {
    name.to_ascii_lowercase().contains("context7")
}

/// A sub-agent runs its own tools; whether their calls surface in the parent's
/// stream is not something Kronn can rely on.
fn is_subagent(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "task" | "agent")
}

/// One simple command of a shell script: its words, and whether the lexer
/// could not read it with certainty (a heredoc, a quote left open).
struct Segment {
    words: Vec<String>,
    opaque: bool,
}

/// Accumulates the words and commands of a shell script.
#[derive(Default)]
struct Lexer {
    commands: Vec<Segment>,
    words: Vec<String>,
    word: String,
    in_word: bool,
    opaque: bool,
    /// Set right after `<<`: the next word is the heredoc delimiter
    /// (`true` when the operator was `<<-`, which strips leading tabs).
    delimiter: Option<bool>,
    /// Heredocs whose body starts on the next line.
    bodies: Vec<(String, bool)>,
}

impl Lexer {
    fn end_word(&mut self) {
        if !self.in_word {
            return;
        }
        self.in_word = false;
        let word = std::mem::take(&mut self.word);
        if let Some(strip) = self.delimiter.take() {
            self.bodies.push((word.clone(), strip));
        }
        self.words.push(word);
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.words.is_empty() || self.opaque {
            self.commands.push(Segment {
                words: std::mem::take(&mut self.words),
                opaque: self.opaque,
            });
        }
        self.opaque = false;
    }
}

/// The simple commands of a shell `script`, split at the control operators
/// (`;` `&` `|` newline, parentheses, backticks). Quotes group a word and are
/// dropped: what a quoted string says is one word, not a command. A `#`
/// comment (through the end of the line) and a heredoc body are data, never
/// commands. A command that holds a heredoc, or a quote left open, is marked
/// opaque: it is not read with certainty.
fn simple_commands(script: &str) -> Vec<Segment> {
    let chars: Vec<char> = script.chars().collect();
    let mut lexer = Lexer::default();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    lexer.word.push(ch);
                }
            }
            Some(_) => match ch {
                '"' => quote = None,
                '\\' => match chars.get(i).copied() {
                    Some(next @ ('"' | '\\' | '$' | '`')) => {
                        lexer.word.push(next);
                        i += 1;
                    }
                    Some(next) => {
                        lexer.word.push('\\');
                        lexer.word.push(next);
                        i += 1;
                    }
                    None => {}
                },
                _ => lexer.word.push(ch),
            },
            None => match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    lexer.in_word = true;
                }
                '\\' => {
                    if let Some(next) = chars.get(i).copied() {
                        i += 1;
                        if next != '\n' {
                            lexer.word.push(next);
                            lexer.in_word = true;
                        }
                    }
                }
                '#' if !lexer.in_word => {
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                }
                '<' if chars.get(i) == Some(&'<') && chars.get(i + 1) != Some(&'<') => {
                    lexer.end_word();
                    lexer.opaque = true;
                    i += 1;
                    let strip = chars.get(i) == Some(&'-');
                    if strip {
                        i += 1;
                    }
                    lexer.delimiter = Some(strip);
                }
                ';' | '&' | '|' | '\n' | '(' | ')' | '`' => {
                    lexer.end_command();
                    if ch == '\n' && !lexer.bodies.is_empty() {
                        for (delimiter, strip) in std::mem::take(&mut lexer.bodies) {
                            while i < chars.len() {
                                let end = chars[i..]
                                    .iter()
                                    .position(|c| *c == '\n')
                                    .map_or(chars.len(), |at| i + at);
                                let line: String = chars[i..end].iter().collect();
                                i = (end + 1).min(chars.len());
                                let line = if strip {
                                    line.trim_start_matches('\t')
                                } else {
                                    line.as_str()
                                };
                                if line == delimiter {
                                    break;
                                }
                            }
                        }
                    }
                }
                c if c.is_whitespace() => lexer.end_word(),
                c => {
                    lexer.word.push(c);
                    lexer.in_word = true;
                }
            },
        }
    }
    if quote.is_some() {
        lexer.opaque = true;
    }
    lexer.end_command();
    lexer.commands
}

#[derive(Clone, Copy, PartialEq)]
enum Downloader {
    Curl,
    Wget,
}

/// What a `curl` / `wget` option takes.
enum OptionKind {
    /// A switch: it takes nothing.
    Flag,
    /// The next word (or the rest of the word it is written in) is its value.
    Value,
    /// It swallows the rest of the word it is written in (`wget -nv`).
    Rest,
}

const CURL_SHORT_FLAGS: &str = "0123456#:aBfgGhiIjJkLMnNOpqRsSvVZ";
const CURL_SHORT_VALUES: &str = "AbcCdDeEFHKmoPQrtTuUwxXyYz";
const WGET_SHORT_FLAGS: &str = "VhbdqvFcNSr46kKpmxEHL";
const WGET_SHORT_VALUES: &str = "eoaiBtOTwQlARDIXPUY";

const CURL_LONG_FLAGS: &[&str] = &[
    "silent",
    "show-error",
    "fail",
    "fail-with-body",
    "fail-early",
    "location",
    "location-trusted",
    "insecure",
    "verbose",
    "include",
    "head",
    "compressed",
    "globoff",
    "get",
    "remote-name",
    "remote-name-all",
    "remote-header-name",
    "remote-time",
    "progress-bar",
    "no-progress-meter",
    "http1.0",
    "http1.1",
    "http2",
    "http2-prior-knowledge",
    "http3",
    "ipv4",
    "ipv6",
    "netrc",
    "no-buffer",
    "raw",
    "tr-encoding",
    "show-headers",
    "create-dirs",
    "parallel",
    "no-keepalive",
    "anyauth",
    "basic",
    "digest",
    "negotiate",
    "ntlm",
    "tlsv1",
    "tlsv1.0",
    "tlsv1.1",
    "tlsv1.2",
    "tlsv1.3",
    "ssl",
    "ssl-reqd",
    "ssl-no-revoke",
    "no-sessionid",
    "retry-connrefused",
    "retry-all-errors",
    "next",
    "no-clobber",
    "junk-session-cookies",
    "post301",
    "post302",
    "post303",
    "path-as-is",
    "styled-output",
    "disable",
    "version",
    "help",
    "manual",
    "append",
    "use-ascii",
    "list-only",
    "proxytunnel",
    "proxy-insecure",
    "tcp-nodelay",
    "tcp-fastopen",
    "ignore-content-length",
    "disallow-username-in-url",
    "trace-time",
];

const CURL_LONG_VALUES: &[&str] = &[
    "data",
    "data-raw",
    "data-binary",
    "data-ascii",
    "data-urlencode",
    "json",
    "header",
    "output",
    "output-dir",
    "request",
    "request-target",
    "user-agent",
    "referer",
    "user",
    "form",
    "form-string",
    "write-out",
    "proxy",
    "proxy-user",
    "proxy-header",
    "preproxy",
    "cookie",
    "cookie-jar",
    "connect-timeout",
    "max-time",
    "retry",
    "retry-delay",
    "retry-max-time",
    "limit-rate",
    "range",
    "upload-file",
    "cacert",
    "capath",
    "cert",
    "cert-type",
    "key",
    "key-type",
    "resolve",
    "connect-to",
    "dump-header",
    "config",
    "oauth2-bearer",
    "max-redirs",
    "max-filesize",
    "time-cond",
    "noproxy",
    "interface",
    "continue-at",
    "ftp-port",
    "stderr",
    "trace",
    "trace-ascii",
    "unix-socket",
    "ciphers",
    "tls13-ciphers",
    "pinnedpubkey",
    "speed-limit",
    "speed-time",
    "keepalive-time",
    "local-port",
    "alt-svc",
    "hsts",
    "doh-url",
    "aws-sigv4",
    "netrc-file",
    "quote",
    "socks4",
    "socks4a",
    "socks5",
    "socks5-hostname",
    "url-query",
    "variable",
];

const WGET_LONG_FLAGS: &[&str] = &[
    "quiet",
    "verbose",
    "no-verbose",
    "debug",
    "continue",
    "timestamping",
    "no-clobber",
    "no-check-certificate",
    "check-certificate",
    "content-disposition",
    "trust-server-names",
    "spider",
    "server-response",
    "no-cache",
    "no-cookies",
    "keep-session-cookies",
    "no-parent",
    "recursive",
    "page-requisites",
    "convert-links",
    "backup-converted",
    "mirror",
    "no-directories",
    "force-directories",
    "no-host-directories",
    "background",
    "inet4-only",
    "inet6-only",
    "show-progress",
    "no-proxy",
    "no-dns-cache",
    "ignore-length",
    "retry-connrefused",
    "no-http-keep-alive",
    "span-hosts",
    "relative",
    "delete-after",
    "html-extension",
    "adjust-extension",
    "no-hsts",
    "no-config",
    "force-html",
    "version",
    "help",
    "no-glob",
    "passive-ftp",
    "content-on-error",
];

const WGET_LONG_VALUES: &[&str] = &[
    "output-document",
    "output-file",
    "append-output",
    "user-agent",
    "referer",
    "header",
    "post-data",
    "post-file",
    "user",
    "password",
    "http-user",
    "http-password",
    "ftp-user",
    "ftp-password",
    "proxy-user",
    "proxy-password",
    "tries",
    "timeout",
    "dns-timeout",
    "connect-timeout",
    "read-timeout",
    "wait",
    "waitretry",
    "limit-rate",
    "directory-prefix",
    "input-file",
    "base",
    "execute",
    "level",
    "accept",
    "reject",
    "accept-regex",
    "reject-regex",
    "domains",
    "exclude-domains",
    "include-directories",
    "exclude-directories",
    "load-cookies",
    "save-cookies",
    "ca-certificate",
    "ca-directory",
    "certificate",
    "certificate-type",
    "private-key",
    "private-key-type",
    "bind-address",
    "method",
    "body-data",
    "body-file",
    "quota",
    "secure-protocol",
    "progress",
    "restrict-file-names",
    "default-page",
];

fn short_option(tool: Downloader, c: char) -> Option<OptionKind> {
    let (flags, values) = match tool {
        Downloader::Curl => (CURL_SHORT_FLAGS, CURL_SHORT_VALUES),
        Downloader::Wget => (WGET_SHORT_FLAGS, WGET_SHORT_VALUES),
    };
    if tool == Downloader::Wget && c == 'n' {
        // `-nv` `-nc` `-nd` `-nH` `-np`: `n` and what follows it is one option.
        Some(OptionKind::Rest)
    } else if flags.contains(c) {
        Some(OptionKind::Flag)
    } else if values.contains(c) {
        Some(OptionKind::Value)
    } else {
        None
    }
}

fn long_option(tool: Downloader, name: &str) -> Option<OptionKind> {
    let (flags, values) = match tool {
        Downloader::Curl => (CURL_LONG_FLAGS, CURL_LONG_VALUES),
        Downloader::Wget => (WGET_LONG_FLAGS, WGET_LONG_VALUES),
    };
    if flags.contains(&name) {
        Some(OptionKind::Flag)
    } else if values.contains(&name) {
        Some(OptionKind::Value)
    } else {
        None
    }
}

/// A shell redirection written as a word: whether its target is the next word
/// (`>` `2>>` `<`) or part of the word (`>out`). `None` for any other word.
fn redirection(word: &str) -> Option<bool> {
    let rest = word.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.starts_with(['<', '>']) {
        Some(rest.trim_start_matches(['<', '>']).is_empty())
    } else {
        None
    }
}

/// An option that makes `curl` / `wget` print something and exit: with it on
/// the command line, whatever else is written (a URL included) transfers
/// nothing.
fn displays_and_exits(tool: Downloader, long: Option<&str>, short: Option<char>) -> bool {
    match tool {
        Downloader::Curl => {
            matches!(long, Some("version" | "help" | "manual"))
                || matches!(short, Some('V' | 'h' | 'M'))
        }
        Downloader::Wget => {
            matches!(long, Some("version" | "help")) || matches!(short, Some('V' | 'h'))
        }
    }
}

/// The options a wrapper command (`sudo`, `env`, `xargs`, `timeout`…) takes
/// before the command it runs.
struct WrapperOptions {
    /// Short switches.
    flags: &'static str,
    /// Short options whose value is the next word (or the rest of their word).
    values: &'static str,
    /// Short options whose value, if any, is written in their own word
    /// (`xargs -i{}`).
    attached: &'static str,
    long_flags: &'static [&'static str],
    long_values: &'static [&'static str],
    /// Long options whose value, if any, follows an `=`.
    long_attached: &'static [&'static str],
    /// `nice -10`: a number is an option.
    numeric: bool,
}

const NO_OPTIONS: WrapperOptions = WrapperOptions {
    flags: "",
    values: "",
    attached: "",
    long_flags: &[],
    long_values: &[],
    long_attached: &[],
    numeric: false,
};

/// The options of a wrapper, or `None` when `name` is not a wrapper this reader
/// looks through. `--help` / `--version` (and any option not listed) are not
/// there: a wrapper invoked with them is not read.
fn wrapper_options(name: &str) -> Option<WrapperOptions> {
    Some(match name {
        "sudo" => WrapperOptions {
            flags: "AbEHnPS",
            values: "CDgpRrTtUu",
            long_flags: &["non-interactive", "stdin", "set-home", "background"],
            long_values: &["user", "group", "chdir", "prompt", "role", "type"],
            long_attached: &["preserve-env"],
            ..NO_OPTIONS
        },
        "env" => WrapperOptions {
            flags: "i0v",
            values: "uC",
            long_flags: &["ignore-environment", "null", "debug"],
            long_values: &["unset", "chdir"],
            ..NO_OPTIONS
        },
        "command" => WrapperOptions {
            flags: "p",
            ..NO_OPTIONS
        },
        "exec" => WrapperOptions {
            flags: "cl",
            values: "a",
            ..NO_OPTIONS
        },
        "nohup" => NO_OPTIONS,
        "time" => WrapperOptions {
            flags: "pav",
            values: "fo",
            long_flags: &["portability", "verbose", "append"],
            long_values: &["format", "output"],
            ..NO_OPTIONS
        },
        "nice" => WrapperOptions {
            values: "n",
            long_values: &["adjustment"],
            numeric: true,
            ..NO_OPTIONS
        },
        "timeout" => WrapperOptions {
            flags: "v",
            values: "ks",
            long_flags: &["foreground", "preserve-status", "verbose"],
            long_values: &["kill-after", "signal"],
            ..NO_OPTIONS
        },
        "xargs" => WrapperOptions {
            flags: "0prtxo",
            values: "adEILnPsJ",
            attached: "ile",
            long_flags: &[
                "null",
                "no-run-if-empty",
                "verbose",
                "interactive",
                "open-tty",
                "exit",
            ],
            long_values: &[
                "arg-file",
                "delimiter",
                "max-args",
                "max-procs",
                "max-chars",
            ],
            long_attached: &["replace", "eof", "max-lines"],
            numeric: false,
        },
        "stdbuf" => WrapperOptions {
            values: "ioe",
            long_values: &["input", "output", "error"],
            ..NO_OPTIONS
        },
        _ => return None,
    })
}

/// The words of a wrapper invocation that follow its options (the command it
/// runs), or `None` when an option is not one this wrapper is known to take: its
/// value could be taken for the command.
fn after_wrapper_options<'a>(
    spec: &WrapperOptions,
    mut rest: &'a [String],
) -> Option<&'a [String]> {
    while let Some(arg) = rest.first().map(String::as_str) {
        if arg == "--" {
            return Some(&rest[1..]);
        }
        if !arg.starts_with('-') {
            return Some(rest);
        }
        rest = &rest[1..];
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let known = (spec.long_flags.contains(&name) && attached.is_none())
                || spec.long_attached.contains(&name);
            if !known {
                if !spec.long_values.contains(&name) {
                    return None;
                }
                if attached.is_none() {
                    rest = rest.get(1..)?;
                }
            }
            continue;
        }
        let cluster = &arg[1..];
        if cluster.is_empty() {
            return None;
        }
        for (at, c) in cluster.char_indices() {
            if spec.flags.contains(c) || (spec.numeric && c.is_ascii_digit()) {
                continue;
            }
            if spec.attached.contains(c) {
                break;
            }
            if spec.values.contains(c) {
                if at + c.len_utf8() == cluster.len() {
                    rest = rest.get(1..)?;
                }
                break;
            }
            return None;
        }
    }
    Some(rest)
}

fn is_http_url(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// The destination URLs of one `curl` / `wget` invocation, or `None` when the
/// invocation is not fully understood: an option this reader does not know (it
/// may take a value, and that value may be a URL that is not a destination), so
/// nothing of it is credited.
fn destination_urls(tool: Downloader, args: &[String]) -> Option<Vec<String>> {
    let mut urls: Vec<&str> = Vec::new();
    let mut positional_only = false;
    let mut next = 0;
    while next < args.len() {
        let arg = args[next].as_str();
        next += 1;
        if positional_only {
            urls.push(arg);
        } else if let Some(target_is_next) = redirection(arg) {
            if target_is_next {
                next += 1;
            }
        } else if arg == "--" {
            positional_only = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            if tool == Downloader::Curl && name == "url" {
                // The destination, as a value.
                match attached {
                    Some(value) => urls.push(value),
                    None => {
                        urls.extend(args.get(next).map(String::as_str));
                        next += 1;
                    }
                }
                continue;
            }
            if displays_and_exits(tool, Some(name), None) {
                return None;
            }
            match long_option(tool, name)? {
                OptionKind::Flag if attached.is_some() => return None,
                OptionKind::Flag => {}
                OptionKind::Value | OptionKind::Rest => {
                    if attached.is_none() {
                        next += 1;
                    }
                }
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            let cluster = &arg[1..];
            for (at, c) in cluster.char_indices() {
                if displays_and_exits(tool, None, Some(c)) {
                    return None;
                }
                match short_option(tool, c)? {
                    OptionKind::Flag => {}
                    OptionKind::Rest => break,
                    OptionKind::Value => {
                        if at + c.len_utf8() == cluster.len() {
                            next += 1;
                        }
                        break;
                    }
                }
            }
        } else {
            urls.push(arg);
        }
    }
    Some(
        urls.into_iter()
            .filter(|word| is_http_url(word))
            .map(normalize_url)
            .filter(|url| !url.is_empty())
            .collect(),
    )
}

/// The pages one simple command downloads. Conservative: a URL is credited
/// only when the command is a real `curl` / `wget` invocation that is fully
/// understood (no heredoc, no quote left open, no option this reader does not
/// know) and the URL is a destination argument of it. A URL that is printed,
/// quoted, a comment, a heredoc line, or the value of an option (`--proxy`,
/// `-o`, `-O`, `--referer`…) is text, not a fetch; a command that is not fully
/// understood credits nothing. Wrappers (`sudo`, `env`, `xargs`…) are looked
/// through, and the script of `sh -c` is read in turn.
fn download_urls(words: &[String], opaque: bool, depth: usize) -> Vec<String> {
    if opaque {
        return Vec::new();
    }
    let mut rest = words;
    let tool = loop {
        let Some(first) = rest.first() else {
            return Vec::new();
        };
        let name = first.rsplit('/').next().unwrap_or(first);
        let assignment = first.split_once('=').is_some_and(|(key, _)| {
            !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if assignment {
            rest = &rest[1..];
            continue;
        }
        if let Some(spec) = wrapper_options(name) {
            // The options of the wrapper are read, values included
            // (`xargs -I curl`: `curl` is a replacement token, not the
            // command). An option it is not known to take, `--help` and
            // `command -v` (a lookup, not a run) among them: nothing is read.
            let Some(after) = after_wrapper_options(&spec, &rest[1..]) else {
                return Vec::new();
            };
            // `timeout` takes the duration before its command.
            rest = if name == "timeout" {
                after.get(1..).unwrap_or(&[])
            } else {
                after
            };
            continue;
        }
        if matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh") {
            let flag = rest
                .iter()
                .position(|w| w.starts_with('-') && !w.starts_with("--") && w.contains('c'));
            return match (flag, depth < 4) {
                (Some(at), true) => rest
                    .get(at + 1)
                    .map(|script| command_urls(script, depth + 1))
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
        }
        break match name {
            "curl" | "curl.exe" => Downloader::Curl,
            "wget" | "wget.exe" => Downloader::Wget,
            _ => return Vec::new(),
        };
    };
    destination_urls(tool, &rest[1..]).unwrap_or_default()
}

/// The pages the `curl` / `wget` invocations of a shell `script` download.
fn command_urls(script: &str, depth: usize) -> Vec<String> {
    simple_commands(script)
        .iter()
        .flat_map(|segment| download_urls(&segment.words, segment.opaque, depth))
        .collect()
}

/// The `command` string of a tool input excerpt that was cut before its JSON
/// closed: the part of the script that is still there.
fn salvaged_command(input: &str) -> Option<String> {
    let tail = input.split("\"command\":").nth(1)?.trim_start();
    let body = tail.strip_prefix('"')?;
    let mut end = body.len();
    let mut escaped = false;
    for (index, ch) in body.char_indices() {
        match (escaped, ch) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, '"') => {
                end = index;
                break;
            }
            _ => {}
        }
    }
    let body = &body[..end];
    Some(serde_json::from_str::<String>(&format!("\"{body}\"")).unwrap_or_else(|_| body.to_owned()))
}

fn url_pattern() -> &'static regex_lite::Regex {
    static PATTERN: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex_lite::Regex::new(r#"https?://[^\s"'<>\\`)\]}|;]+"#).expect("url pattern compiles")
    })
}

/// Every normalized `http(s)` URL written in `text`.
pub(crate) fn urls_in(text: &str) -> Vec<String> {
    url_pattern()
        .find_iter(text)
        .map(|m| normalize_url(m.as_str().trim_end_matches(['.', ',', ':'])))
        .filter(|u| !u.is_empty())
        .collect()
}

/// The text of a tool result: a string, or the `text` blocks of an array.
pub(crate) fn result_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The pages one tool call reads, from its structured input: the `url` of a
/// page opener (`opens`: a fetch or a browser navigation), the URLs of a
/// `curl` / `wget` command. Read from the whole input, before any excerpt.
pub(crate) fn input_urls(opens: bool, input: &serde_json::Value) -> Vec<String> {
    let mut urls = Vec::new();
    if opens {
        let single = input.get("url").and_then(|u| u.as_str());
        let many = input
            .get("urls")
            .and_then(|u| u.as_array())
            .into_iter()
            .flatten()
            .filter_map(|u| u.as_str());
        urls.extend(
            single
                .into_iter()
                .chain(many)
                .map(normalize_url)
                .filter(|u| !u.is_empty()),
        );
    }
    match input.get("command") {
        Some(serde_json::Value::Array(parts)) => {
            let words: Vec<String> = parts
                .iter()
                .filter_map(|part| part.as_str().map(str::to_owned))
                .collect();
            urls.extend(download_urls(&words, false, 0));
        }
        Some(serde_json::Value::String(script)) => urls.extend(command_urls(script, 0)),
        _ => {}
    }
    urls
}

/// The pages one tool call reads, from the input text it reported. A bounded
/// excerpt may be cut: the `url` or the command it still holds is salvaged.
fn call_urls(name: &str, input: &str) -> Vec<String> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(input) {
        return input_urls(opens_url(name), &json);
    }
    let mut urls = Vec::new();
    if opens_url(name) {
        let url = input
            .split("\"url\":\"")
            .nth(1)
            .and_then(|tail| tail.split('"').next());
        urls.extend(url.map(normalize_url).filter(|u| !u.is_empty()));
    }
    if let Some(script) = salvaged_command(input) {
        urls.extend(command_urls(&script, 0));
    }
    urls
}

/// The URLs fetched by a CLI agent that streams its tool calls
/// (Claude stream-json): a call starts, its input arrives in fragments, it ends.
#[derive(Debug, Clone)]
pub struct FetchTracker {
    open: Option<(String, String)>,
    /// Ids of the documentation calls whose result is read for URLs.
    docs_calls: BTreeSet<String>,
    urls: BTreeSet<String>,
    observable: bool,
}

impl Default for FetchTracker {
    fn default() -> Self {
        Self {
            open: None,
            docs_calls: BTreeSet::new(),
            urls: BTreeSet::new(),
            observable: true,
        }
    }
}

impl FetchTracker {
    pub fn tool_started(&mut self, name: &str) {
        self.open = Some((name.to_owned(), String::new()));
    }

    pub fn tool_input(&mut self, fragment: &str) {
        if let Some((_, input)) = self.open.as_mut() {
            if input.len() + fragment.len() <= MAX_TOOL_INPUT_BYTES {
                input.push_str(fragment);
            }
        }
    }

    pub fn tool_ended(&mut self) {
        if let Some((name, input)) = self.open.take() {
            self.record(&name, &input);
        }
    }

    fn record(&mut self, name: &str, input: &str) {
        if is_subagent(name) {
            self.observable = false;
        }
        self.urls.extend(call_urls(name, input));
    }

    /// One raw stream-json line: the complete tool calls of an `assistant`
    /// message, and the result a documentation tool (Context7) sent back to the
    /// agent, whose pages are the ones the agent read.
    pub fn ingest_line(&mut self, line: &str) {
        if !line.contains("\"tool_use\"") && !line.contains("\"tool_result\"") {
            return;
        }
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            return;
        };
        let Some(blocks) = json.pointer("/message/content").and_then(|c| c.as_array()) else {
            return;
        };
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let input = block
                        .get("input")
                        .map(|i| i.to_string())
                        .unwrap_or_default();
                    self.record(name, &input);
                    if let (true, Some(id)) = (
                        returns_urls(name),
                        block.get("id").and_then(|id| id.as_str()),
                    ) {
                        self.docs_calls.insert(id.to_owned());
                    }
                }
                Some("tool_result") => {
                    let held = block
                        .get("tool_use_id")
                        .and_then(|id| id.as_str())
                        .is_some_and(|id| self.docs_calls.contains(id));
                    if held {
                        let text = block.get("content").map(result_text).unwrap_or_default();
                        self.urls.extend(urls_in(&text));
                    }
                }
                _ => {}
            }
        }
    }

    /// The fetches an ACP-hosted agent (Claude and Codex through their adapters,
    /// the native ACP agents) reported in the tool traces its process captured.
    /// A trace carries the pages its call opened, and the pages a documentation
    /// tool returned (read where the result is seen, never the result itself).
    /// A documentation call whose result was not seen leaves the pages the
    /// agent read unknown: the run's `url:` citations are then left unchecked.
    pub fn ingest_traces(&mut self, captured: &[String]) {
        for trace in crate::agents::tool_trace::collect(captured) {
            let Some(name) = trace.name.as_deref() else {
                self.urls.extend(trace.urls);
                continue;
            };
            // A lookup whose result was seen and named no page (Context7's
            // `resolve-library-id`) is no blind spot; one never seen is.
            if returns_urls(name) && trace.urls.is_empty() && !trace.result_seen {
                self.observable = false;
            }
            self.urls.extend(trace.urls.iter().cloned());
            self.record(name, &trace.input.unwrap_or_default());
        }
    }

    /// An agent whose tool inputs are not reported was run: from now on the
    /// fetches of the run are not known.
    pub fn mark_unobservable(&mut self) {
        self.observable = false;
    }

    /// The fetches to check against, `None` when they are not all known.
    pub fn fetched(&self) -> Option<BTreeSet<String>> {
        self.observable.then(|| self.urls.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::version_sources::WatchedPackage;
    use serde_json::json;

    fn watch() -> DependencySnapshot {
        DependencySnapshot {
            checked_on: "2026-10-16".into(),
            packages: ["argparse", "joi", "qs"]
                .iter()
                .map(|name| WatchedPackage {
                    manager: "JS / TS".into(),
                    manifest: "package.json".into(),
                    name: (*name).into(),
                    current: "1.0.0".into(),
                    latest: "2.0.0".into(),
                    major: true,
                    declared_only: false,
                    lockfile: None,
                })
                .collect(),
            unchecked: vec![],
            up_to_date: vec![],
        }
    }

    fn provenance(fetched: &[&str]) -> RunProvenance {
        let mut tracker = FetchTracker::default();
        for url in fetched {
            tracker.tool_started("WebFetch");
            tracker.tool_input(&format!("{{\"url\":\"{url}\",\"prompt\":\"v\"}}"));
            tracker.tool_ended();
        }
        RunProvenance {
            deps: Some(watch()),
            fetched: tracker.fetched(),
        }
    }

    #[test]
    fn an_invented_latest_stable_with_fake_provenance_is_refused() {
        let table = "| Node | 23.0.0 [src: url: https://nodejs.org/en/about/previous-releases] (read 2026-10-16) |\n\
                     | Jest | 30.0.0 [src: deps: JS / TS/jest@2026-10-16] |\n";
        let problems = provenance(&[]).problems(table);
        assert_eq!(problems.len(), 2, "{problems:#?}");
        assert!(problems[0].contains("never fetched"), "{}", problems[0]);
        assert!(problems[0].contains("not verified"), "{}", problems[0]);
        assert!(
            problems[1].contains("not in the dependency watch"),
            "{}",
            problems[1]
        );
    }

    #[test]
    fn held_and_fetched_citations_pass() {
        let table = "joi lags [src: deps: JS / TS/joi@2026-10-16]; \
                     Node 22 [src: url: https://NodeJS.org/en/about/previous-releases/#lts] (read 2026-10-16)";
        let provenance = provenance(&["https://nodejs.org/en/about/previous-releases"]);
        assert!(provenance.problems(table).is_empty());
    }

    #[test]
    fn not_verified_and_quoted_examples_pass() {
        let text = "| Prettier | not verified: no source |\n\
                    Grammar: `[src: url: https://example.com]` and\n\
                    ```\n[src: deps: npm/ghost@2026-10-16]\n```\n";
        assert!(provenance(&[]).problems(text).is_empty());
    }

    #[test]
    fn a_run_with_no_dependency_watch_backs_no_deps_citation() {
        let mut run = provenance(&[]);
        run.deps = None;
        assert_eq!(
            run.problems("Jest [src: deps: JS / TS/jest@2026-10-16]")
                .len(),
            1
        );
    }

    #[test]
    fn url_citations_are_left_alone_when_the_fetches_are_not_observable() {
        let mut tracker = FetchTracker::default();
        tracker.mark_unobservable();
        let run = RunProvenance {
            deps: Some(watch()),
            fetched: tracker.fetched(),
        };
        assert!(run
            .problems("Node [src: url: https://nodejs.org/en/about] (read 2026-10-16)")
            .is_empty());
    }

    #[test]
    fn the_tracker_reads_only_fetch_tools_and_their_url() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("Read");
        tracker.tool_input("{\"url\":\"https://not-a-fetch.dev\"}");
        tracker.tool_ended();
        tracker.tool_started("mcp__fetch__fetch");
        tracker.tool_input("{\"ur");
        tracker.tool_input("l\":\"http://Example.com/a/\"}");
        tracker.tool_ended();
        tracker.tool_ended();
        assert_eq!(
            tracker.fetched().unwrap().into_iter().collect::<Vec<_>>(),
            vec!["example.com/a".to_string()]
        );
    }

    #[test]
    fn the_tracker_reads_the_fetches_of_an_acp_hosted_agent_from_its_traces() {
        let trace = |id: &str, name: &str, input: &str| {
            crate::agents::tool_trace::ToolTraceUpdate {
                id: id.into(),
                name: Some(name.into()),
                input: Some(input.into()),
                status: Some("in_progress".into()),
                urls: vec![],
                result_seen: false,
            }
            .marker()
        };
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[
            trace(
                "a",
                "WebFetch",
                r#"{"prompt":"p","url":"https://Nodejs.org/en/"}"#,
            ),
            trace("b", "Read", r#"{"url":"https://not-a-fetch.dev"}"#),
            trace(
                "c",
                "WebFetch",
                r#"{"prompt":"cut","url":"https://x.dev/a/b"#,
            ),
            "plain stderr line".into(),
        ]);
        assert_eq!(
            tracker.fetched().unwrap().into_iter().collect::<Vec<_>>(),
            vec!["nodejs.org/en".to_string(), "x.dev/a/b".to_string()]
        );
    }

    #[test]
    fn url_citation_forms_resolve_to_one_address() {
        assert_eq!(
            url_reference("url: https://a.dev/x (read 2026-10-16)").as_deref(),
            Some("https://a.dev/x")
        );
        assert_eq!(
            url_reference("https://a.dev/x").as_deref(),
            Some("https://a.dev/x")
        );
        assert_eq!(url_reference("file: src/a.rs:1"), None);
        assert_eq!(normalize_url("HTTPS://A.dev/x/#top"), "a.dev/x");
    }

    fn cites(url: &str) -> String {
        format!("Node 22 [src: url: {url}] (read 2026-10-16)")
    }

    fn run_of(tracker: &FetchTracker) -> RunProvenance {
        RunProvenance::of(&Some(watch()), tracker)
    }

    fn assistant_call(id: &str, name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{input}}}]}}}}"#
        )
    }

    fn user_result(id: &str, content: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","content":{content}}}]}}}}"#
        )
    }

    #[test]
    fn a_page_a_context7_result_names_is_a_page_the_run_read() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_line(&assistant_call(
            "t1",
            "mcp__context7__query-docs",
            r#"{"libraryId":"/vercel/next.js","query":"app router"}"#,
        ));
        tracker.ingest_line(&user_result(
            "t1",
            r#"[{"type":"text","text":"Routing\nSource: https://nextjs.org/docs/app/building-your-application/routing\nSee [x](https://react.dev/reference/react)."}]"#,
        ));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites(
                "https://nextjs.org/docs/app/building-your-application/routing"
            ))
            .is_empty());
        assert!(run
            .problems(&cites("https://react.dev/reference/react"))
            .is_empty());
        // A page the result did not name is still invented.
        assert_eq!(run.problems(&cites("https://nodejs.org/en")).len(), 1);
    }

    #[test]
    fn only_a_documentation_tool_result_is_read_for_urls() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_line(&assistant_call("r1", "Read", r#"{"file_path":"/a.md"}"#));
        tracker.ingest_line(&user_result("r1", r#""see https://seen-in-a-file.dev/a""#));
        assert_eq!(
            run_of(&tracker)
                .problems(&cites("https://seen-in-a-file.dev/a"))
                .len(),
            1
        );
    }

    #[test]
    fn a_browser_navigation_is_a_fetch() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("mcp__Playwright__browser_navigate");
        tracker.tool_input(r#"{"url":"https://nodejs.org/en/about/previous-releases"}"#);
        tracker.tool_ended();
        tracker.ingest_line(&assistant_call(
            "d1",
            "mcp__Chrome DevTools__navigate_page",
            r#"{"type":"url","url":"https://vitest.dev/guide/"}"#,
        ));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://nodejs.org/en/about/previous-releases"))
            .is_empty());
        assert!(run.problems(&cites("https://vitest.dev/guide")).is_empty());
    }

    #[test]
    fn a_curl_or_wget_command_is_a_fetch() {
        let mut tracker = FetchTracker::default();
        tracker.tool_started("Bash");
        tracker.tool_input(r#"{"command":"curl -sL 'https://nodejs.org/dist/index.json' | head"}"#);
        tracker.tool_ended();
        tracker.ingest_line(&assistant_call(
            "b1",
            "Bash",
            r#"{"command":"wget -qO- https://registry.npmjs.org/jest/latest"}"#,
        ));
        // Codex reports the command as an array.
        tracker.record(
            "command_execution",
            r#"{"command":["curl","https://endoflife.date/api/node.json"]}"#,
        );
        // A command that downloads nothing names no page it read.
        tracker.ingest_line(&assistant_call(
            "b2",
            "Bash",
            r#"{"command":"echo https://example.com/not-fetched"}"#,
        ));
        let run = run_of(&tracker);
        for url in [
            "https://nodejs.org/dist/index.json",
            "https://registry.npmjs.org/jest/latest",
            "https://endoflife.date/api/node.json",
        ] {
            assert!(run.problems(&cites(url)).is_empty(), "{url}");
        }
        assert_eq!(
            run.problems(&cites("https://example.com/not-fetched"))
                .len(),
            1
        );
    }

    #[test]
    fn a_leading_www_does_not_make_another_page() {
        assert_eq!(normalize_url("https://www.Nodejs.org/en/"), "nodejs.org/en");
        let mut tracker = FetchTracker::default();
        tracker.tool_started("WebFetch");
        tracker.tool_input(r#"{"url":"https://www.typescriptlang.org/docs"}"#);
        tracker.tool_ended();
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://typescriptlang.org/docs"))
            .is_empty());
        assert!(run
            .problems(&cites("https://www.typescriptlang.org/docs/"))
            .is_empty());
    }

    #[test]
    fn a_sub_agent_leaves_the_url_citations_unchecked() {
        for name in ["Task", "Agent"] {
            let mut tracker = FetchTracker::default();
            tracker.ingest_line(&assistant_call(
                "s1",
                name,
                r#"{"description":"look up the LTS","prompt":"fetch it"}"#,
            ));
            assert!(
                run_of(&tracker)
                    .problems(&cites("https://nodejs.org/en"))
                    .is_empty(),
                "{name}"
            );
            // The dependency watch is still the run's own.
            assert_eq!(
                run_of(&tracker)
                    .problems("Jest [src: deps: JS / TS/jest@2026-10-16]")
                    .len(),
                1
            );
        }
    }

    #[test]
    fn an_acp_run_reads_browsers_and_commands_and_cannot_see_a_docs_result() {
        let trace = |id: &str, name: &str, input: &str| {
            crate::agents::tool_trace::ToolTraceUpdate {
                id: id.into(),
                name: Some(name.into()),
                input: Some(input.into()),
                status: Some("in_progress".into()),
                urls: vec![],
                result_seen: false,
            }
            .marker()
        };
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[
            trace(
                "a",
                "mcp__playwright__browser_navigate",
                r#"{"url":"https://vitest.dev/guide"}"#,
            ),
            trace(
                "b",
                "command_execution",
                r#"{"command":"curl -s https://nodejs.org/dist/index.json"}"#,
            ),
        ]);
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://vitest.dev/guide")).is_empty());
        assert!(run
            .problems(&cites("https://nodejs.org/dist/index.json"))
            .is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io")).len(), 1);

        tracker.ingest_traces(&[trace(
            "c",
            "mcp__context7__query-docs",
            r#"{"query":"jest"}"#,
        )]);
        assert!(run_of(&tracker)
            .problems(&cites("https://jestjs.io"))
            .is_empty());
    }

    /// The traces the ACP adapter would capture for these Claude lines.
    fn claude_traces(lines: &[String]) -> Vec<String> {
        let mut reader = crate::agents::tool_trace::ClaudeTraceReader::default();
        lines
            .iter()
            .flat_map(|line| reader.read(line))
            .map(|trace| trace.marker())
            .collect()
    }

    #[test]
    fn a_context7_result_url_survives_the_acp_trace() {
        let page = "https://nextjs.org/docs/app/routing";
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "t1",
                "mcp__context7__query-docs",
                r#"{"libraryId":"/vercel/next.js","query":"routing"}"#,
            ),
            user_result(
                "t1",
                &format!(r#"[{{"type":"text","text":"Source: {page}\nmore"}}]"#),
            ),
            // A result of another tool is not read for URLs.
            assistant_call("t2", "Read", r#"{"file_path":"/a.md"}"#),
            user_result("t2", r#""see https://seen-in-a-file.dev/a""#),
        ]));
        let run = run_of(&tracker);
        assert!(run.fetched.is_some(), "the result was seen");
        assert!(run.problems(&cites(page)).is_empty());
        assert_eq!(
            run.problems(&cites("https://seen-in-a-file.dev/a")).len(),
            1
        );
        assert_eq!(run.problems(&cites("https://react.dev")).len(), 1);
    }

    #[test]
    fn a_trace_keeps_the_url_of_an_input_the_excerpt_cut() {
        let long = "x".repeat(5000);
        let fetch = format!(r#"{{"url":"https://a.dev/page","prompt":"{long}"}}"#);
        let shell = format!(r#"{{"command":"echo {long} && curl -s https://b.dev/api"}}"#);
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call("f1", "WebFetch", &fetch),
            assistant_call("f2", "Bash", &shell),
        ]));
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://a.dev/page")).is_empty());
        assert!(run.problems(&cites("https://b.dev/api")).is_empty());
        assert_eq!(run.problems(&cites("https://c.dev")).len(), 1);
    }

    #[test]
    fn a_native_acp_fetch_is_known_by_its_kind_even_retitled_to_its_url() {
        use crate::agents::tool_trace::from_acp;
        let frames = [
            json!({"toolCallId":"n1","kind":"fetch","title":"Fetch","status":"pending",
                   "rawInput":{"url":"https://www.x.dev/docs"}}),
            // The runtime retitles the call to the page it opened.
            json!({"toolCallId":"n1","title":"https://x.dev/docs","status":"completed"}),
            json!({"toolCallId":"n2","kind":"fetch","title":"https://y.dev/a?b=1",
                   "status":"in_progress"}),
            json!({"toolCallId":"n3","kind":"read","title":"Read",
                   "rawInput":{"url":"https://not-a-fetch.dev"}}),
        ];
        let captured: Vec<String> = frames
            .iter()
            .map(|frame| from_acp(frame).unwrap().marker())
            .collect();
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&captured);
        let run = run_of(&tracker);
        assert!(run.problems(&cites("https://x.dev/docs")).is_empty());
        assert!(run.problems(&cites("https://y.dev/a?b=1")).is_empty());
        assert_eq!(run.problems(&cites("https://not-a-fetch.dev")).len(), 1);
    }

    #[test]
    fn a_codex_context7_result_url_survives_the_trace() {
        let item = json!({"id":"c1","type":"mcp_tool_call","server":"context7",
            "tool":"query-docs","arguments":{"query":"jest"},"status":"completed",
            "result":{"content":[{"type":"text","text":"Docs: https://jestjs.io/docs/api\nend"}]}});
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[crate::agents::tool_trace::from_codex(&item)
            .unwrap()
            .marker()]);
        let run = run_of(&tracker);
        assert!(run.fetched.is_some());
        assert!(run
            .problems(&cites("https://jestjs.io/docs/api"))
            .is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io/blog")).len(), 1);
    }

    #[test]
    fn a_resolved_library_id_names_no_page_and_is_no_blind_spot() {
        let page = "https://jestjs.io/docs/getting-started";
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "r1",
                "mcp__context7__resolve-library-id",
                r#"{"libraryName":"jest","query":"latest"}"#,
            ),
            user_result(
                "r1",
                r#"[{"type":"text","text":"- Title: Jest\n- Context7-compatible library ID: /facebook/jest\n- Code Snippets: 120\n- Benchmark Score: 91"}]"#,
            ),
            assistant_call(
                "q1",
                "mcp__context7__query-docs",
                r#"{"libraryId":"/facebook/jest","query":"latest"}"#,
            ),
            user_result(
                "q1",
                &format!(r#"[{{"type":"text","text":"Source: {page}"}}]"#),
            ),
        ]));
        let run = run_of(&tracker);
        assert!(run.fetched.is_some(), "the resolve result was seen");
        assert!(run.problems(&cites(page)).is_empty());
        assert_eq!(run.problems(&cites("https://jestjs.io/blog")).len(), 1);

        // A resolve alone: still checked, the invented page is refused.
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call(
                "r1",
                "mcp__context7__resolve-library-id",
                r#"{"libraryName":"jest"}"#,
            ),
            user_result(
                "r1",
                r#""- Context7-compatible library ID: /facebook/jest""#,
            ),
        ]));
        assert_eq!(
            run_of(&tracker)
                .problems(&cites("https://jestjs.io/blog"))
                .len(),
            1
        );
    }

    #[test]
    fn a_documentation_call_whose_result_never_arrived_leaves_the_citations_unchecked() {
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[assistant_call(
            "q1",
            "mcp__context7__query-docs",
            r#"{"query":"jest"}"#,
        )]));
        assert!(run_of(&tracker).fetched.is_none());
    }

    #[test]
    fn a_native_acp_resolve_with_a_seen_result_is_no_blind_spot() {
        use crate::agents::tool_trace::from_acp;
        let frame = json!({"toolCallId":"n1","title":"mcp__context7__resolve-library-id",
            "status":"completed","rawOutput":{"id":"/facebook/jest","snippets":120}});
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[from_acp(&frame).unwrap().marker()]);
        assert!(run_of(&tracker).fetched.is_some());
        let unseen = json!({"toolCallId":"n2","title":"mcp__context7__resolve-library-id",
            "status":"in_progress"});
        tracker.ingest_traces(&[from_acp(&unseen).unwrap().marker()]);
        assert!(run_of(&tracker).fetched.is_none());
    }

    #[test]
    fn a_url_keeps_no_userinfo_and_no_signature() {
        assert_eq!(
            normalize_url("https://deploy:hunter2@Files.example.com/a.json"),
            "files.example.com/a.json"
        );
        assert_eq!(
            normalize_url("https://u@x.dev?z=1"),
            "x.dev?z=1",
            "a query with no path"
        );
        let signed = "https://bucket.s3.amazonaws.com/a.json?X-Amz-Algorithm=AWS4-HMAC-SHA256\
            &X-Amz-Credential=AKIAFAKE%2F20261018&X-Amz-Signature=deadbeef01&version=3";
        let n = normalize_url(signed);
        assert!(!n.contains("deadbeef01") && !n.contains("AKIAFAKE"), "{n}");
        assert!(n.contains("version=3"), "{n}");
        for (url, secret) in [
            (
                "https://a.blob.core.windows.net/c/f?sv=1&sig=s3cr3tsig",
                "s3cr3tsig",
            ),
            ("https://a.dev/f?access_token=tok123abc", "tok123abc"),
            ("https://a.dev/f?api-key=key123abc&page=2", "key123abc"),
        ] {
            assert!(!normalize_url(url).contains(secret), "{url}");
        }
        // Two spellings of one signed page still compare equal.
        assert_eq!(
            normalize_url("https://a.dev/f?sig=one&v=1"),
            normalize_url("https://www.a.dev/f?sig=two&v=1")
        );
    }

    #[test]
    fn the_stored_trace_urls_hold_no_secret() {
        let mut tracker = FetchTracker::default();
        let lines = claude_traces(&[
            assistant_call(
                "f1",
                "WebFetch",
                r#"{"url":"https://ci:hunter2@dl.example.com/a?sig=abc123secret&v=2","prompt":"x"}"#,
            ),
            assistant_call("t1", "mcp__context7__query-docs", r#"{"query":"q"}"#),
            user_result(
                "t1",
                r#""Docs https://docs.example.com/x?X-Amz-Signature=feedface99&p=1""#,
            ),
        ]);
        // The URL list the trace records (the input excerpt is redacted by its
        // own, older rules).
        let stored: String = crate::agents::tool_trace::collect(&lines)
            .iter()
            .flat_map(|trace| trace.urls.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!stored.is_empty());
        for secret in ["hunter2", "abc123secret", "feedface99"] {
            assert!(!stored.contains(secret), "{secret} in {stored}");
        }
        tracker.ingest_traces(&lines);
        let run = run_of(&tracker);
        // The citation, written with or without the secret, matches the page.
        assert!(run
            .problems(&cites("https://dl.example.com/a?sig=other&v=2"))
            .is_empty());
        assert!(run
            .problems(&cites("https://docs.example.com/x?X-Amz-Signature=zz&p=1"))
            .is_empty());
    }

    #[test]
    fn a_large_documentation_result_still_backs_a_late_citation() {
        let text: String = (0..200)
            .map(|i| format!("Page https://docs.example.com/p/{i}\n"))
            .collect();
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&claude_traces(&[
            assistant_call("q1", "mcp__context7__query-docs", r#"{"query":"all"}"#),
            user_result("q1", &json!([{"type":"text","text":text}]).to_string()),
        ]));
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://docs.example.com/p/199"))
            .is_empty());
        assert_eq!(
            run.problems(&cites("https://docs.example.com/p/200")).len(),
            1
        );
    }

    /// The URLs a `command_execution` of Codex records as read.
    fn read_by_command(command: serde_json::Value) -> RunProvenance {
        let item = json!({"id": "c", "type": "command_execution", "command": command,
            "status": "completed", "exit_code": 0});
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&[crate::agents::tool_trace::from_codex(&item)
            .unwrap()
            .marker()]);
        run_of(&tracker)
    }

    #[test]
    fn a_printed_or_quoted_curl_is_text_not_a_fetch() {
        let url = "https://never-read.invalid/latest-stable";
        for command in [
            format!("printf '%s\\n' 'Documentation example: curl {url}'"),
            format!("echo curl {url}"),
            format!("echo \"wget {url}\""),
            format!("cat notes.md # curl {url}"),
            format!("grep -n curl {url}"),
            format!("git clone {url}"),
            format!("curl -d {url} https://api.example.com/post"),
        ] {
            let run = read_by_command(json!(command));
            assert_eq!(run.problems(&cites(url)).len(), 1, "{command}");
        }
    }

    #[test]
    fn a_real_curl_or_wget_invocation_is_a_fetch() {
        let page = "https://download.example.com/releases";
        for command in [
            json!(format!("curl -sL {page}")),
            json!(format!("curl --url={page} -o out.html")),
            json!(format!("cd /tmp && wget -q '{page}' && ls")),
            json!(format!("sudo -E env A=1 curl {page} | head")),
            json!(format!("FOO=1 /usr/bin/curl -H 'X: y' \"{page}\"")),
            json!(format!("cat urls | xargs curl {page}")),
            json!(["/bin/zsh", "-lc", format!("cd repo && curl -s {page}")]),
            json!(["curl", "-s", page]),
            json!(format!("bash -c \"echo hi; wget {page}\"")),
        ] {
            let run = read_by_command(command.clone());
            assert!(run.problems(&cites(page)).is_empty(), "{command}");
        }
    }

    #[test]
    fn a_cut_command_excerpt_keeps_the_download_it_still_holds() {
        let mut tracker = FetchTracker::default();
        tracker.record(
            "Bash",
            r#"{"command":"cd /repo && curl -sL https://files.example.com/a.json -o a.json && echo this is cut off befo"#,
        );
        let run = run_of(&tracker);
        assert!(run
            .problems(&cites("https://files.example.com/a.json"))
            .is_empty());
        let mut printed = FetchTracker::default();
        printed.record(
            "Bash",
            r#"{"command":"echo 'curl https://never.example.com/x' && sleep 1 and the rest is cut"#,
        );
        assert_eq!(
            run_of(&printed)
                .problems(&cites("https://never.example.com/x"))
                .len(),
            1
        );
    }

    /// The provenance of a shell `command` on both routes: Codex through the
    /// ACP trace, Claude through the direct stream.
    fn read_by_command_on_both_routes(
        label: &str,
        command: &str,
    ) -> [(&'static str, RunProvenance); 2] {
        let acp = read_by_command(json!(command));
        let mut tracker = FetchTracker::default();
        tracker.ingest_line(&assistant_call(
            label,
            "Bash",
            &json!({ "command": command }).to_string(),
        ));
        tracker.ingest_line(&user_result(label, r#""fixture command completed""#));
        [("Codex/ACP", acp), ("Claude/direct", run_of(&tracker))]
    }

    #[test]
    fn only_a_destination_argument_of_a_fully_understood_download_counts() {
        let page = "https://download.example.test/current";
        let noise = "https://never-read.invalid/only-text";
        // (label, command, the page is credited, the noise URL is credited)
        let cases = [
            ("plain curl", format!("curl -fsSL '{page}'"), true, false),
            ("wget", format!("wget -qO- '{page}'"), true, false),
            (
                "absolute curl",
                format!("/usr/bin/curl -s '{page}'"),
                true,
                false,
            ),
            (
                "env wrapper",
                format!("env LANG=C curl -s '{page}'"),
                true,
                false,
            ),
            (
                "sudo wrapper",
                format!("sudo -n curl -s '{page}'"),
                true,
                false,
            ),
            (
                "assignment prefix",
                format!("LANG=C curl -s '{page}'"),
                true,
                false,
            ),
            ("url flag", format!("curl --url '{page}'"), true, false),
            ("pipeline", format!("curl -s '{page}' | cat"), true, false),
            (
                "printed example",
                format!("printf '%s\\n' 'Documentation example: curl {noise}'"),
                false,
                false,
            ),
            (
                "comment only",
                format!("# curl {noise}\nprintf ok"),
                false,
                false,
            ),
            (
                "other command",
                format!("curl -s '{page}'; printf '%s\\n' '{noise}'"),
                true,
                false,
            ),
            (
                "trailing comment",
                format!("curl -s '{page}' # {noise}"),
                true,
                false,
            ),
            (
                "header argument",
                format!("curl -H 'X-Documentation: {noise}' '{page}'"),
                true,
                false,
            ),
            (
                "request data",
                format!("curl --data 'source={noise}' '{page}'"),
                true,
                false,
            ),
            (
                "referer argument",
                format!("curl --referer '{noise}' '{page}'"),
                true,
                false,
            ),
            (
                "wget referer",
                format!("wget --referer='{noise}' '{page}'"),
                true,
                false,
            ),
            (
                "output filename",
                format!("curl --output '{noise}' '{page}'"),
                true,
                false,
            ),
            (
                "variable mentions curl",
                format!("EXAMPLE=curl printf '%s\\n' '{noise}'"),
                false,
                false,
            ),
            (
                "command inside comment",
                format!("printf ok # ; curl '{noise}'"),
                false,
                false,
            ),
            (
                "heredoc example",
                format!("cat <<'DOC'\ncurl '{noise}'\nDOC"),
                false,
                false,
            ),
            (
                "proxy option",
                format!("curl --proxy '{noise}' '{page}'"),
                true,
                false,
            ),
            (
                "wget output filename",
                format!("wget -O '{noise}' '{page}'"),
                true,
                false,
            ),
        ];
        let mut mismatches = Vec::new();
        for (label, command, page_credited, noise_credited) in &cases {
            for (route, run) in read_by_command_on_both_routes(label, command) {
                for (url, expected) in [(page, page_credited), (noise, noise_credited)] {
                    let accepted = run.problems(&cites(url)).is_empty();
                    if accepted != *expected {
                        mismatches.push(format!(
                            "{label}/{route}: accepted={accepted}, expected={expected}, url={url}"
                        ));
                    }
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }

    #[test]
    fn a_command_that_is_not_fully_understood_credits_nothing() {
        let page = "https://download.example.test/current";
        let noise = "https://never-read.invalid/only-text";
        for command in [
            // An option this reader does not know may take a URL as its value.
            format!("curl --some-future-option '{noise}' '{page}'"),
            format!("curl -Y '{noise}' --not-an-option '{page}'"),
            format!("wget --proxy '{noise}' '{page}'"),
            // A short option it does not know, inside a cluster.
            format!("curl -sJ@ '{page}'"),
            // A quote left open: the end of the command is not known.
            format!("curl -s '{page}' 'unterminated"),
            // A heredoc feeding the download itself.
            format!("curl -s '{page}' <<EOF\nbody\nEOF"),
            // A lookup, not a run.
            format!("command -v curl {page}"),
        ] {
            for (route, run) in read_by_command_on_both_routes("opaque", &command) {
                for url in [page, noise] {
                    assert_eq!(
                        run.problems(&cites(url)).len(),
                        1,
                        "{route}: {command} must credit {url} nothing"
                    );
                }
            }
        }
    }

    #[test]
    fn a_display_and_exit_mode_transfers_nothing() {
        let page = "https://download.example.test/current";
        for command in [
            format!("curl --version '{page}'"),
            format!("curl -V '{page}'"),
            format!("curl -sV '{page}'"),
            format!("curl '{page}' --version"),
            format!("curl --help '{page}'"),
            format!("curl -h '{page}'"),
            format!("curl --manual '{page}'"),
            format!("curl -M '{page}'"),
            format!("wget --version '{page}'"),
            format!("wget -V '{page}'"),
            format!("wget --help '{page}'"),
            format!("wget -h '{page}'"),
            format!("sudo curl --version '{page}'"),
        ] {
            for (route, run) in read_by_command_on_both_routes("display", &command) {
                assert_eq!(
                    run.problems(&cites(page)).len(),
                    1,
                    "{route}: {command} must credit {page} nothing"
                );
            }
        }
        // After `--`, a word that looks like the option is a positional.
        let after_separator = format!("curl -s -- '{page}'");
        for (route, run) in read_by_command_on_both_routes("separator", &after_separator) {
            assert!(run.problems(&cites(page)).is_empty(), "{route}");
        }
    }

    #[test]
    fn a_wrapper_option_value_is_not_the_command() {
        let page = "https://download.example.test/current";
        for command in [
            // `-I` takes `curl` as its replacement token: `printf` is the command.
            format!("printf x | xargs -I curl printf '%s\\n' '{page}'"),
            format!("printf x | xargs -Icurl printf '%s\\n' '{page}'"),
            format!("printf x | xargs --arg-file curl printf '%s\\n' '{page}'"),
            format!("sudo -u curl printf '%s\\n' '{page}'"),
            format!("nice -n curl printf '%s\\n' '{page}'"),
            format!("env -u curl printf '%s\\n' '{page}'"),
            format!("timeout -s curl 5 printf '%s\\n' '{page}'"),
            format!("stdbuf -o curl printf '%s\\n' '{page}'"),
            // A wrapper option this reader does not know, or a mode that runs nothing.
            format!("sudo --version curl '{page}'"),
            format!("sudo -V curl '{page}'"),
            format!("sudo --some-future-option curl '{page}'"),
            format!("env -S 'curl {page}'"),
            format!("env --version curl '{page}'"),
            format!("nohup --help curl '{page}'"),
            format!("timeout --help curl '{page}'"),
            format!("xargs --show-limits curl '{page}'"),
            format!("xargs -Y curl '{page}'"),
            format!("nice -z curl '{page}'"),
        ] {
            for (route, run) in read_by_command_on_both_routes("wrapper", &command) {
                assert_eq!(
                    run.problems(&cites(page)).len(),
                    1,
                    "{route}: {command} must credit {page} nothing"
                );
            }
        }
    }

    #[test]
    fn a_wrapper_with_known_options_still_runs_the_download() {
        let page = "https://download.example.test/current";
        for command in [
            format!("xargs -n1 curl -s '{page}'"),
            format!("printf x | xargs -I{{}} curl -s '{page}'"),
            format!("printf x | xargs -I @ -P 2 curl -s '{page}'"),
            format!("xargs -i curl -s '{page}'"),
            format!("sudo -n -u root curl -s '{page}'"),
            format!("sudo -E -- curl -s '{page}'"),
            format!("env -i -u HOME LANG=C curl -s '{page}'"),
            format!("nohup curl -s '{page}'"),
            format!("nice curl -s '{page}'"),
            format!("nice -n 10 curl -s '{page}'"),
            format!("nice -10 curl -s '{page}'"),
            format!("timeout 30 curl -s '{page}'"),
            format!("timeout -k 5 --signal=TERM 30 curl -s '{page}'"),
            format!("stdbuf -oL curl -s '{page}'"),
            format!("time -p wget -q '{page}'"),
            format!("command -p curl -s '{page}'"),
            format!("exec -a name curl -s '{page}'"),
        ] {
            for (route, run) in read_by_command_on_both_routes("known", &command) {
                assert!(run.problems(&cites(page)).is_empty(), "{route}: {command}");
            }
        }
    }

    #[test]
    fn a_heredoc_or_a_comment_hides_text_but_not_the_commands_around_it() {
        let page = "https://download.example.test/current";
        let noise = "https://never-read.invalid/only-text";
        let script = format!(
            "cat <<-EOF\n\tcurl '{noise}'\n\tEOF\ncurl -s '{page}' # curl {noise}\necho ok"
        );
        for (route, run) in read_by_command_on_both_routes("mixed", &script) {
            assert!(run.problems(&cites(page)).is_empty(), "{route}");
            assert_eq!(run.problems(&cites(noise)).len(), 1, "{route}");
        }
    }

    #[test]
    fn a_redirection_target_and_a_wget_cluster_are_read_as_such() {
        let page = "https://download.example.test/current";
        let noise = "https://never-read.invalid/only-text";
        for command in [
            format!("curl -s '{page}' > '{noise}'"),
            format!("curl -s '{page}' 2>'{noise}'"),
            format!("wget -nv -q -P /tmp '{page}'"),
            format!("wget -nv '{page}' >/dev/null"),
        ] {
            let run = read_by_command(json!(command));
            assert!(run.problems(&cites(page)).is_empty(), "{command}");
            assert_eq!(run.problems(&cites(noise)).len(), 1, "{command}");
        }
    }

    #[test]
    fn an_encoded_credential_name_is_masked_in_the_persisted_trace() {
        for (key, secret) in [
            ("%73ig", "fixture-encoded-sig"),
            ("access%5Ftoken", "fixture-encoded-token"),
            ("X-Amz-%53ignature", "fixture-encoded-signature"),
            ("api%2Dkey", "fixture-encoded-key"),
            ("%2573ig", "fixture-double-encoded"),
            ("sig%3Dfixture-encoded-eq", "fixture-encoded-eq"),
            ("API%5fKEY", "fixture-upper-key"),
        ] {
            let url = if key.contains("%3D") {
                format!("https://docs.example.com/release?{key}&page=2")
            } else {
                format!("https://docs.example.com/release?{key}={secret}&page=2")
            };
            let lines = claude_traces(&[
                assistant_call("q", "mcp__context7__query-docs", r#"{"query":"x"}"#),
                user_result("q", &json!(url).to_string()),
            ]);
            let stored: Vec<String> = crate::agents::tool_trace::collect(&lines)
                .iter()
                .flat_map(|trace| trace.urls.clone())
                .collect();
            let stored = stored.join("\n");
            assert!(!stored.contains(secret), "{key}: {stored}");
            assert!(stored.contains("page=2"), "{key}: a plain param is kept");
            assert!(stored.contains("=***"), "{key}: {stored}");
        }
        assert_eq!(
            normalize_url("https://a.dev/f?note=a%26b&x%3D1=2"),
            "a.dev/f?note=a%26b&x%3D1=2",
            "a plain parameter is kept as written"
        );
    }

    #[test]
    fn one_canonical_form_on_the_stored_and_the_cited_side() {
        let source = "https://user:fixture-password@docs.example.test/v3?X-Amz-Credential=fixture-credential\
            &X-Amz-Signature=fixture-signature&access_token=fixture-token&page=2";
        let lines = claude_traces(&[
            assistant_call("q", "mcp__context7__query-docs", r#"{"query":"x"}"#),
            user_result("q", &json!(source).to_string()),
        ]);
        let stored = lines.join("\n");
        for secret in [
            "fixture-password",
            "fixture-credential",
            "fixture-signature",
            "fixture-token",
        ] {
            assert!(!stored.contains(secret), "{stored}");
        }
        let mut tracker = FetchTracker::default();
        tracker.ingest_traces(&lines);
        let run = run_of(&tracker);
        let fetched = run.fetched.clone().unwrap();
        assert_eq!(
            fetched.into_iter().collect::<Vec<_>>(),
            vec![normalize_url(source)],
            "the stored form is the cited form, secrets masked and `page` kept"
        );
        assert!(run.problems(&cites(source)).is_empty());
        assert!(run
            .problems(&cites(&source.replace("fixture-signature", "renewed")))
            .is_empty());
        assert_eq!(
            run.problems(&cites(&source.replace("page=2", "page=3")))
                .len(),
            1
        );
        // Normalizing a stored form changes nothing.
        let once = normalize_url(source);
        assert_eq!(normalize_url(&once), once);
    }
}
