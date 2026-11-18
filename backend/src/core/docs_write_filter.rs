//! Anti-secret filter for agent writes targeting the project's docs/ tree.
//!
//! When an agent (Claude Code, Codex, …) writes to `docs/<subfolder>/X.md` to
//! capture project memory, it occasionally drags in content that should
//! NEVER end up committed: dumped `.env` lines, API tokens, SSH key
//! fragments, JWT bodies, etc. Agents have no built-in sense of
//! confidentiality — and the consequences are durable (git history,
//! PR review, multi-user share). Once a leak lands, it's painful.
//!
//! This module provides a pure-logic filter the runner calls on every
//! agent-modified file under the project's docs directory at step end.
//! Auditing never restores or deletes files. The workflow stops on rejected
//! changes, preserving the working tree and index for human inspection.
//!
//! ## Detection layers
//!
//! 1. **Memory-entry size cap** : `check_docs_write` retains the small-entry
//!    contract. The workflow audit checks project documents without this cap:
//!    a large architecture document is not evidence of a leak.
//! 2. **Regex denylist** : well-known secret prefixes (sk-, ghp_, AKIA,
//!    xox[bapr]-, …) and structural markers (PEM headers, JWT shapes).
//! 3. **High-entropy detector** : a 32+ char run of base64/hex mixing
//!    letters and digits. Its form cannot tell a token from an identifier
//!    (a dated id, a hash a document cites), so the workflow audit rejects
//!    nothing on it: it reports the lines as possible secrets and the step
//!    stands (KT-1796). `check_docs_write` still rejects on it.
//! 4. **Substring match against sensitive worktree files** : on entry
//!    the runner Bloom-prefixes the content of `.env*`, `*.pem`, `*.key`,
//!    `id_rsa*`, `credentials*`, `.aws/`, `.ssh/`. A write that contains
//!    any matching substring (≥ 12 chars) is rejected — catches the
//!    "agent grepped my .env then wrote a doc about it" failure mode.

use std::collections::HashSet;
use std::path::Path;

/// Hard size cap for an entry. A memory fact rarely exceeds 2 KB; 8 KB
/// is a conservative ceiling that still catches accidental log dumps.
pub const MAX_ENTRY_BYTES: usize = 8 * 1024;

/// Substring length below which Bloom-check matches don't trigger a
/// reject. Prevents false positives on common short tokens (`PATH`,
/// `HOME`, etc. that happen to appear in env files).
const BLOOM_MIN_SUBSTRING_LEN: usize = 12;

/// High-entropy run length that triggers the entropy detector. 32 chars
/// of dense base64 ≈ 192 bits — well into "this is a token" territory.
const HIGH_ENTROPY_MIN_LEN: usize = 32;

/// Reasons a write to `docs/` can be rejected. Caller decides what to do
/// (fail the step and surface a secret-free diagnostic in run detail).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRejection {
    /// Write exceeds `MAX_ENTRY_BYTES`. Field is the actual size.
    TooLarge { bytes: usize },
    /// Regex denylist matched. Field is the pattern that triggered; `line`
    /// is 1-based, like every `line` below.
    DenylistPattern { pattern: &'static str, line: usize },
    /// 32+ char high-entropy run detected. The run itself is never kept.
    HighEntropyRun { line: usize },
    /// Content matches a substring of a known sensitive worktree file.
    /// Field is the relative path of the source file.
    SensitiveFileSubstring { source: String, line: usize },
}

impl SecretRejection {
    /// Human-readable explanation for logs / UI.
    pub fn explain(&self) -> String {
        match self {
            SecretRejection::TooLarge { bytes } => {
                format!(
                    "write rejected: {} bytes exceeds the {}-byte memory cap",
                    bytes, MAX_ENTRY_BYTES
                )
            }
            SecretRejection::DenylistPattern { pattern, line } => {
                format!("write rejected: line {line} matches secret pattern {pattern:?}")
            }
            SecretRejection::HighEntropyRun { line } => format!(
                "write rejected: line {line} has a 32+ char high-entropy run (possible credential; content withheld)"
            ),
            SecretRejection::SensitiveFileSubstring { source, line } => format!(
                "write rejected: line {line} overlaps with a substring from sensitive file {source:?}"
            ),
        }
    }
}

/// Every detector, the size cap first. Returns on first reject. A memory
/// entry has no reader to warn: a possible secret is rejected too.
pub fn check_docs_write(
    content: &str,
    sensitive_substrings: &SensitiveSubstrings,
) -> Result<(), SecretRejection> {
    if content.len() > MAX_ENTRY_BYTES {
        return Err(SecretRejection::TooLarge {
            bytes: content.len(),
        });
    }
    check_docs_content(content, sensitive_substrings)?;
    match possible_secret_lines(content).first() {
        Some(&line) => Err(SecretRejection::HighEntropyRun { line }),
        None => Ok(()),
    }
}

/// A project document is not a small memory entry. Neither its length nor
/// the form of a long run is evidence of a leak: only a known secret format
/// or a value of a sensitive file rejects it. `possible_secret_lines` reports
/// the rest (KT-1796).
fn check_docs_content(
    content: &str,
    sensitive_substrings: &SensitiveSubstrings,
) -> Result<(), SecretRejection> {
    if let Some((pattern, offset)) = match_denylist(content) {
        let line = line_of(content, offset);
        return Err(SecretRejection::DenylistPattern { pattern, line });
    }
    if let Some((source, offset)) = sensitive_substrings.find(content) {
        let line = line_of(content, offset);
        return Err(SecretRejection::SensitiveFileSubstring { source, line });
    }
    if let Some((source, offset)) = sensitive_substrings.shared(content) {
        let line = line_of(content, offset);
        return Err(SecretRejection::SensitiveFileSubstring { source, line });
    }
    Ok(())
}

/// The 1-based lines holding a high-entropy run, each once, in order.
fn possible_secret_lines(content: &str) -> Vec<usize> {
    // One pass: the offsets come in order.
    let bytes = content.as_bytes();
    let (mut line, mut at) = (1, 0);
    let mut lines: Vec<usize> = Vec::new();
    for offset in high_entropy_runs(content) {
        line += bytes[at..offset].iter().filter(|b| **b == b'\n').count();
        at = offset;
        if lines.last() != Some(&line) {
            lines.push(line);
        }
    }
    lines
}

/// 1-based line of a byte offset.
fn line_of(content: &str, offset: usize) -> usize {
    content.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Regex denylist match. Returns the matched pattern name and the byte
/// offset of the match on hit.
pub fn match_denylist(content: &str) -> Option<(&'static str, usize)> {
    static PATTERNS: &[(&str, &str)] = &[
        // Well-known token prefixes (vendor-attributable).
        ("sk-...", r"\bsk-[A-Za-z0-9_\-]{20,}"),
        ("ghp_/gho_/ghu_/ghs_/ghr_", r"\bgh[opusr]_[A-Za-z0-9]{30,}"),
        ("AKIA (AWS)", r"\bAKIA[0-9A-Z]{16}\b"),
        ("Slack xox[bapr]-", r"\bxox[baprs]-[A-Za-z0-9-]{10,}"),
        (
            "Stripe rk_/sk_live",
            r"\b(rk_live_|sk_live_)[A-Za-z0-9]{20,}",
        ),
        ("Atlassian ATATT", r"\bATATT[A-Za-z0-9_=\-]{30,}"),
        // Generic credential keywords with a value attached.
        (
            "api_key=...",
            r#"(?i)api[_-]?key\s*[:=]\s*["'][A-Za-z0-9_\-]{8,}"#,
        ),
        ("password=...", r#"(?i)password\s*[:=]\s*["'][^"']{4,}"#),
        (
            "token=...",
            r#"(?i)\btoken\s*[:=]\s*["'][A-Za-z0-9_\-\.]{16,}"#,
        ),
        (
            "secret=...",
            r#"(?i)\bsecret\s*[:=]\s*["'][A-Za-z0-9_\-]{8,}"#,
        ),
        ("bearer ...", r"(?i)bearer\s+[A-Za-z0-9_\-\.]{20,}"),
        // Structural markers — JWT, PEM, RSA private key.
        (
            "JWT body",
            r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}",
        ),
        ("PEM private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        ("OpenSSH private", r"-----BEGIN OPENSSH PRIVATE KEY-----"),
    ];
    static COMPILED: std::sync::OnceLock<Vec<(&str, regex_lite::Regex)>> =
        std::sync::OnceLock::new();
    let compiled = COMPILED.get_or_init(|| {
        PATTERNS
            .iter()
            .filter_map(|(name, regex)| Some((*name, regex_lite::Regex::new(regex).ok()?)))
            .collect()
    });
    compiled
        .iter()
        .find_map(|(name, re)| Some((*name, re.find(content)?.start())))
}

/// The byte offset of every contiguous run of ≥ 32 base64-or-hex characters
/// that mixes letters and digits.
fn high_entropy_runs(content: &str) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    let bytes = content.as_bytes();
    for i in 0..=bytes.len() {
        if bytes.get(i).is_some_and(|&b| is_token_char(b as char)) {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            let run = &content[s..i];
            if is_long_mixed_run(run) {
                runs.push(s);
            }
        }
    }
    runs
}

/// A file yields the same substring twice in a row (a line, then the run
/// it is made of): looked for among the last entries only.
const DEDUP_WINDOW: usize = 8;

/// A sensitive file beyond this size is not a credential file.
const MAX_SENSITIVE_FILE_BYTES: usize = 4 * 1024 * 1024;

/// The readings of a sensitive file a document can quote from: its text,
/// the same without NUL bytes (UTF-16), and each with the escapes of a JSON
/// string undone (a key stored on one line).
fn sensitive_texts(bytes: &[u8]) -> Vec<String> {
    let mut texts = vec![String::from_utf8_lossy(bytes).into_owned()];
    if bytes.contains(&0) {
        let narrow: Vec<u8> = bytes.iter().copied().filter(|&b| b != 0).collect();
        texts.push(String::from_utf8_lossy(&narrow).into_owned());
    }
    let unescaped: Vec<String> = texts
        .iter()
        .filter(|text| text.contains('\\'))
        .map(|text| undo_json_escapes(text))
        .collect();
    texts.extend(unescaped);
    texts
}

/// `\n`, `\r`, `\/` and the `\uXXXX` of an ASCII character, as a JSON
/// serialiser writes them inside a string.
fn undo_json_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('\\') {
        plain.push_str(&rest[..at]);
        let escape = &rest[at..];
        let ascii = escape
            .get(2..6)
            .filter(|_| escape.as_bytes().get(1) == Some(&b'u'))
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .filter(u8::is_ascii);
        let (undone, length) = match (escape.as_bytes().get(1), ascii) {
            (_, Some(ascii)) => (ascii as char, 6),
            (Some(b'n'), _) => ('\n', 2),
            (Some(b'r'), _) => ('\r', 2),
            (Some(b'/'), _) => ('/', 2),
            _ => ('\\', 1),
        };
        plain.push(undone);
        rest = &escape[length..];
    }
    plain.push_str(rest);
    plain
}

/// A run long and mixed enough to be a token, as a whole or in a part
/// between `=` or `/`: a name glued to a value (`API_KEY=…`) does not
/// dilute it.
fn is_long_mixed_run(run: &str) -> bool {
    let long_mixed =
        |part: &str| part.len() >= HIGH_ENTROPY_MIN_LEN && mixes_digits_and_letters(part);
    long_mixed(run) || run.split('=').any(long_mixed) || run.split(['=', '/']).any(long_mixed)
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '+' | '=')
}

/// Both classes present, and digits ≥ 15% to rule out plain English /
/// kebab-case identifiers.
fn mixes_digits_and_letters(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut digits = 0;
    let mut alphas = 0;
    for &b in bytes {
        if b.is_ascii_digit() {
            digits += 1;
        } else if b.is_ascii_alphabetic() {
            alphas += 1;
        }
    }
    let total = bytes.len();
    if total == 0 {
        return false;
    }
    let digit_ratio = (digits as f32) / (total as f32);
    digits > 0 && alphas > 0 && digit_ratio >= 0.15
}

/// Bloom-style check : holds a set of substrings extracted from sensitive
/// worktree files, and lets the filter check if a write contains any.
///
/// We don't actually use a Bloom filter (HashSet is fine for the small N
/// of substrings we're tracking — typically a few hundred lines of env /
/// pem). The "Bloom" naming references the design intent of "fast
/// substring presence check at scale", in case we later swap impl.
pub struct SensitiveSubstrings {
    /// Map: substring → file it came from. The substring is the value
    /// part of an `KEY=VALUE` line, the body of a PEM block, etc.
    /// Caller pre-extracts these on run start.
    entries: Vec<(String, String)>,
    /// The text of each sensitive file, with the file it came from: a long
    /// run of a document found in one is a known secret.
    texts: Vec<(String, String)>,
    /// Every 32-character window of the long runs of `texts`, hashed: what
    /// a document shares with one, whatever it is glued to.
    windows: HashSet<u64>,
}

/// What a document and a sensitive file share reads as a value, not as a
/// rule, a name or a path: six different characters, and either a digit
/// with a letter or letters whose case keeps changing.
fn reads_as_a_value(shared: &[u8]) -> bool {
    let mut seen = [false; 256];
    shared.iter().for_each(|&b| seen[b as usize] = true);
    let case_changes = shared
        .windows(2)
        .filter(|pair| {
            pair[0].is_ascii_alphabetic()
                && pair[1].is_ascii_alphabetic()
                && pair[0].is_ascii_uppercase() != pair[1].is_ascii_uppercase()
        })
        .count();
    let mixed = shared.iter().any(u8::is_ascii_digit) && shared.iter().any(u8::is_ascii_alphabetic);
    seen.iter().filter(|seen| **seen).count() >= 6 && (mixed || case_changes >= 8)
}

fn window_hash(window: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    window.hash(&mut hasher);
    hasher.finish()
}

impl SensitiveSubstrings {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            texts: Vec::new(),
            windows: HashSet::new(),
        }
    }

    /// Track the text of a sensitive file.
    fn add_text(&mut self, text: String, source: &str) {
        let bytes = text.as_bytes();
        let mut start = 0;
        for end in 0..=bytes.len() {
            if bytes.get(end).is_some_and(|&b| is_token_char(b as char)) {
                continue;
            }
            if end - start >= HIGH_ENTROPY_MIN_LEN {
                for window in bytes[start..end].windows(HIGH_ENTROPY_MIN_LEN) {
                    self.windows.insert(window_hash(window));
                }
            }
            start = end + 1;
        }
        self.texts.push((text, source.to_string()));
    }

    /// The sensitive file sharing with `content` a stretch that reads as a
    /// value (`reads_as_a_value`), and where. A stretch is every character
    /// covered by 32 in a row that a sensitive file holds, one after the
    /// other: a value whole, glued to a name, cut short or extended, held
    /// in one place of a file or across several.
    fn shared(&self, content: &str) -> Option<(String, usize)> {
        if self.windows.is_empty() {
            return None;
        }
        let bytes = content.as_bytes();
        let mut start = 0;
        for end in 0..=bytes.len() {
            if bytes.get(end).is_some_and(|&b| is_token_char(b as char)) {
                continue;
            }
            // Windows of this run; one turn more closes a stretch at its end.
            let windows = (end + 1 - start).saturating_sub(HIGH_ENTROPY_MIN_LEN);
            // First window of the stretch being followed.
            let mut first: Option<usize> = None;
            for at in start..=start + windows {
                let held = at < start + windows
                    && self
                        .windows
                        .contains(&window_hash(&bytes[at..at + HIGH_ENTROPY_MIN_LEN]));
                match (first, held) {
                    (None, true) => first = Some(at),
                    (Some(from), false) => {
                        first = None;
                        let stretch = from..at - 1 + HIGH_ENTROPY_MIN_LEN;
                        if !reads_as_a_value(&bytes[stretch.clone()]) {
                            continue;
                        }
                        // The hash only sorts: a text names the file.
                        let source = (from..at).find_map(|window| {
                            let window = &content[window..window + HIGH_ENTROPY_MIN_LEN];
                            let held = self.texts.iter().find(|(text, _)| text.contains(window));
                            held.map(|(_, source)| source.clone())
                        });
                        if let Some(source) = source {
                            return Some((source, from));
                        }
                    }
                    _ => {}
                }
            }
            start = end + 1;
        }
        None
    }

    /// Everything `other` tracks, added to this.
    pub fn extend_from(&mut self, other: &SensitiveSubstrings) {
        let known: HashSet<&str> = self.entries.iter().map(|(sub, _)| sub.as_str()).collect();
        let new: Vec<(String, String)> = other
            .entries
            .iter()
            .filter(|(sub, _)| !known.contains(sub.as_str()))
            .cloned()
            .collect();
        self.entries.extend(new);
        let known: HashSet<&str> = self.texts.iter().map(|(text, _)| text.as_str()).collect();
        let new: Vec<(String, String)> = other
            .texts
            .iter()
            .filter(|(text, _)| !known.contains(text.as_str()))
            .cloned()
            .collect();
        self.texts.extend(new);
        self.windows.extend(&other.windows);
    }

    /// Add a substring associated with a source path.
    pub fn add(&mut self, substring: impl Into<String>, source: impl Into<String>) {
        let s = substring.into();
        if s.len() < BLOOM_MIN_SUBSTRING_LEN {
            return; // ignore short tokens — too false-positive-prone
        }
        if self
            .entries
            .iter()
            .rev()
            .take(DEDUP_WINDOW)
            .any(|(known, _)| *known == s)
        {
            return;
        }
        self.entries.push((s, source.into()));
    }

    /// Check if `content` contains any tracked sensitive substring.
    /// Returns the source path of the first match.
    pub fn matches(&self, content: &str) -> Option<String> {
        self.find(content).map(|(source, _)| source)
    }

    /// `matches`, with the byte offset of the match.
    fn find(&self, content: &str) -> Option<(String, usize)> {
        self.entries.iter().find_map(|(sub, src)| {
            content
                .find(sub.as_str())
                .map(|offset| (src.clone(), offset))
        })
    }

    /// Number of tracked substrings (mostly for telemetry / tests).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for SensitiveSubstrings {
    fn default() -> Self {
        Self::new()
    }
}

/// Walk a worktree and extract values from sensitive files. The runner
/// calls this once per run start; the resulting `SensitiveSubstrings`
/// is reused across all docs/ writes within that run.
///
/// Targets : `.env*`, `*.pem`, `*.key`, `id_rsa*`, `credentials*`, plus
/// known credential paths (`.aws/credentials`, `.ssh/config`, etc.). We
/// extract the VALUE side of `KEY=VALUE` lines (env-style), the body of
/// PEM blocks, and any line ≥ `BLOOM_MIN_SUBSTRING_LEN` chars in raw
/// credential files.
pub fn scan_sensitive_files(worktree: &Path) -> SensitiveSubstrings {
    let mut subs = SensitiveSubstrings::new();
    let mut visited: HashSet<std::path::PathBuf> = HashSet::new();

    walk_worktree(worktree, &mut visited, &mut |path: &Path| {
        let rel = path
            .strip_prefix(worktree)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_sensitive = name.starts_with(".env")
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.starts_with("id_rsa")
            || name.starts_with("id_ed25519")
            || name.starts_with("credentials")
            || rel.contains(".aws/")
            || rel.contains(".ssh/");
        if !is_sensitive {
            return;
        }
        // Whatever the syntax or the encoding around it, a long run of a
        // sensitive file is a known secret: the audit only warns of the
        // runs it finds in none of them.
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() <= MAX_SENSITIVE_FILE_BYTES {
                for text in sensitive_texts(&bytes) {
                    subs.add_text(text, &rel);
                }
            }
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                // KEY=VALUE — keep VALUE.
                if let Some((_k, v)) = trimmed.split_once('=') {
                    let v = v.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
                    if v.len() >= BLOOM_MIN_SUBSTRING_LEN {
                        subs.add(v, &rel);
                        continue;
                    }
                }
                // Otherwise, keep the raw line if long enough.
                if trimmed.len() >= BLOOM_MIN_SUBSTRING_LEN {
                    subs.add(trimmed, &rel);
                }
            }
        }
    });

    subs
}

/// Content identities, independent of HEAD/index status. No document contents
/// or credentials are retained in the snapshot.
#[derive(Default)]
pub struct DocsSnapshot {
    files: std::collections::BTreeMap<std::path::PathBuf, [u8; 32]>,
}

const MAX_AUDIT_BUFFER_BYTES: usize = 16 * 1024 * 1024;

struct DocumentRead {
    fingerprint: [u8; 32],
    content: Option<Vec<u8>>,
}

fn read_document(path: &Path, retain_content: bool) -> std::io::Result<Option<DocumentRead>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut hash = Sha256::new();
    let mut content = retain_content.then(Vec::new);
    let mut buffer = [0u8; 32 * 1024];
    loop {
        let count = match file.read(&mut buffer) {
            Ok(count) => count,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        if let Some(bytes) = content.as_mut() {
            if bytes.len().saturating_add(count) <= MAX_AUDIT_BUFFER_BYTES {
                bytes.extend_from_slice(&buffer[..count]);
            } else {
                content = None;
            }
        }
    }
    Ok(Some(DocumentRead {
        fingerprint: hash.finalize().into(),
        content,
    }))
}

/// Git -z emits native path bytes on Unix. Never decode them lossily for I/O.
fn native_document_path(raw: &[u8]) -> Result<std::path::PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(std::ffi::OsStr::from_bytes(raw).into())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(raw)
            .map(std::path::PathBuf::from)
            .map_err(|_| "Document path is not valid UTF-8; audit incomplete".to_string())
    }
}

/// Human-readable, project-relative diagnostic only; never used to open a file.
/// Escape the entire byte representation when a component needs escaping, so
/// distinct invalid names and literal backslashes cannot collapse to one label.
fn document_path_label(path: &Path) -> String {
    let parts: Vec<_> = path.components().map(|part| part.as_os_str()).collect();
    let plain: Option<Vec<_>> = parts.iter().map(|part| part.to_str()).collect();
    if let Some(plain) = plain {
        if plain
            .iter()
            .all(|part| !part.chars().any(|c| c == '\\' || c.is_control()))
        {
            return plain.join("/");
        }
    }
    let escaped = parts
        .iter()
        .map(|part| {
            part.as_encoded_bytes()
                .iter()
                .flat_map(|byte| byte.escape_ascii())
                .map(char::from)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("[escaped path bytes] {escaped}")
}

fn document_paths(worktree: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let docs_dir = crate::core::scanner::detect_docs_dir(worktree);
    let docs_rel = docs_dir
        .strip_prefix(worktree)
        .map_err(|_| "Document directory is outside its workspace".to_string())?;
    let output = crate::core::lane_git::git(worktree)?
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .arg(docs_rel)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("Cannot enumerate versioned documents: {e}"))?;
    if !output.status.success() {
        // The previous Git-status audit also did not inspect non-Git projects.
        if String::from_utf8_lossy(&output.stderr).starts_with("fatal: not a git repository") {
            return Ok(Vec::new());
        }
        return Err("Cannot enumerate documents with git ls-files; audit incomplete".into());
    }
    let mut paths = std::collections::BTreeSet::new();
    for raw in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let path = native_document_path(raw)?;
        if !path.starts_with(docs_rel)
            || path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("Invalid project-relative document path".into());
        }
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("AGENTS.md" | "index.md")
        ) {
            continue;
        }
        // Check each component: a tracked directory can have been replaced by
        // a symlink after checkout. Never traverse that link during the audit.
        let mut current = worktree.to_path_buf();
        let mut regular = true;
        for component in path.components() {
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    regular = false;
                    break;
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    regular = false;
                    break;
                }
                Err(e) => {
                    return Err(format!(
                        "Cannot inspect document {}: {e}",
                        document_path_label(&path)
                    ))
                }
            }
        }
        if regular && current.is_file() {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().collect())
}

/// Snapshot regular project documents without following symbolic links.
/// This is an observation boundary, not proof of which process wrote a file.
pub async fn snapshot_docs(worktree: &Path) -> Result<DocsSnapshot, String> {
    let worktree = worktree.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut snapshot = DocsSnapshot::default();
        for relative in document_paths(&worktree)? {
            if let Some(document) =
                read_document(&worktree.join(&relative), false).map_err(|e| {
                    format!(
                        "Cannot fingerprint document {}: {e}",
                        document_path_label(&relative)
                    )
                })?
            {
                snapshot.files.insert(relative, document.fingerprint);
            }
        }
        Ok(snapshot)
    })
    .await
    .map_err(|e| format!("Document snapshot interrupted: {e}"))?
}

/// What an audit found in the documents a step changed.
#[derive(Debug, Default)]
pub struct DocsAudit {
    /// Known secrets: the caller must fail the step.
    pub rejections: Vec<(String, SecretRejection)>,
    /// Per document, the lines holding a possible secret. They fail nothing:
    /// the caller tells the reader, who decides (KT-1796). The line a
    /// rejection names is not listed again.
    pub possible_secrets: Vec<(String, Vec<usize>)>,
    /// Every document the step changed or removed: what the caller knew of
    /// them before is replaced, what it knew of the others still holds.
    pub inspected: Vec<String>,
    /// Why some changed documents could not be read whole: the caller must
    /// fail the step, after telling what the others hold.
    pub incomplete: Vec<String>,
}

/// Inspect only content changed since the pre-step snapshot. Never mutate the
/// worktree or index: another process may have authored any observed change.
/// The caller must fail the step on rejection or an incomplete audit, and
/// show the possible secrets.
pub async fn audit_docs_writes(
    worktree: &Path,
    sensitive: &SensitiveSubstrings,
    before: &DocsSnapshot,
) -> Result<DocsAudit, String> {
    let mut audit = DocsAudit::default();
    let mut present = HashSet::new();
    let root = worktree.to_path_buf();
    let paths = tokio::task::spawn_blocking(move || document_paths(&root))
        .await
        .map_err(|e| format!("Document enumeration interrupted: {e}"))??;
    for path in paths {
        let absolute = worktree.join(&path);
        let document = tokio::task::spawn_blocking(move || read_document(&absolute, true))
            .await
            .map_err(|e| format!("Document read interrupted: {e}"))?;
        // One document that cannot be read never hides what the others hold.
        let document = match document {
            Ok(Some(document)) => document,
            Ok(None) => continue,
            Err(e) => {
                audit.incomplete.push(format!(
                    "Cannot audit changed document {}: {e}",
                    document_path_label(&path)
                ));
                present.insert(path);
                continue;
            }
        };
        present.insert(path.clone());
        if before.files.get(&path) == Some(&document.fingerprint) {
            continue;
        }
        let Some(bytes) = document.content else {
            audit.incomplete.push(format!(
                "Changed document {} exceeds the 16 MiB audit buffer; content was not inspected and the file was preserved",
                document_path_label(&path)
            ));
            continue;
        };
        let content = document_text(&bytes);
        let content = content.as_ref();
        audit.inspected.push(document_path_label(&path));
        // Only the first known secret is named: every line worth a look is
        // told, the rejected one too (it can hold another run).
        let lines = possible_secret_lines(content);
        if let Err(reason) = check_docs_content(content, sensitive) {
            audit.rejections.push((document_path_label(&path), reason));
        }
        if !lines.is_empty() {
            audit
                .possible_secrets
                .push((document_path_label(&path), lines));
        }
    }
    // Gone since the snapshot: what was known of them no longer holds.
    let removed = before.files.keys().filter(|path| !present.contains(*path));
    audit
        .inspected
        .extend(removed.map(|path| document_path_label(path)));
    Ok(audit)
}

/// The text of a document as its reader sees it: a byte-order mark and NUL
/// bytes dropped (UTF-16), bytes that are no UTF-8 replaced. Lines keep
/// their numbers. No document is set aside as binary: what tells text from
/// an image also hides text in another script or a pasted terminal capture.
fn document_text(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    let bytes = match bytes {
        [0xff, 0xfe, rest @ ..] | [0xfe, 0xff, rest @ ..] => rest,
        _ => bytes,
    };
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\0') {
        return text.replace('\0', "").into();
    }
    text
}

/// Recursively walk a directory, calling `cb` on each FILE. Skips
/// `.git/`, `node_modules/`, `target/`, `vendor/` to keep scanning
/// fast on large repos.
fn walk_worktree(dir: &Path, visited: &mut HashSet<std::path::PathBuf>, cb: &mut dyn FnMut(&Path)) {
    if !visited.insert(dir.to_path_buf()) {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(
            name.as_str(),
            ".git" | "node_modules" | "target" | "vendor" | ".kronn" | "dist" | "build"
        ) {
            continue;
        }
        if path.is_dir() {
            walk_worktree(&path, visited, cb);
        } else if path.is_file() {
            cb(&path);
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn fixture_git(root: &Path, args: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("user.name=Audit Test")
            .arg("-c")
            .arg("user.email=audit@example.invalid")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    #[tokio::test]
    async fn audit_preserves_preexisting_tracked_staged_and_untracked_documents() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("docs")).unwrap();
        fixture_git(root, &["init", "-q"]);
        std::fs::write(root.join("docs/architecture.md"), "committed version\n").unwrap();
        fixture_git(root, &["add", "docs/architecture.md"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        let staged = "staged human architecture\n".repeat(600);
        std::fs::write(root.join("docs/architecture.md"), &staged).unwrap();
        fixture_git(root, &["add", "docs/architecture.md"]);
        let working = format!("{staged}uncommitted human paragraph\n");
        let untracked = "untracked release map\n".repeat(600);
        std::fs::write(root.join("docs/architecture.md"), &working).unwrap();
        std::fs::write(root.join("docs/release map.tsv"), &untracked).unwrap();
        let index_before = fixture_git(root, &["show", ":docs/architecture.md"]);
        let status_before = fixture_git(root, &["status", "--porcelain=v1", "-uall"]);
        let before = snapshot_docs(root).await.unwrap();
        let rejected = audit_rejections(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert!(
            rejected.is_empty(),
            "preexisting writes are not owned by this step"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("docs/architecture.md")).unwrap(),
            working
        );
        assert_eq!(
            std::fs::read_to_string(root.join("docs/release map.tsv")).unwrap(),
            untracked
        );
        assert_eq!(
            fixture_git(root, &["show", ":docs/architecture.md"]),
            index_before
        );
        assert_eq!(
            fixture_git(root, &["status", "--porcelain=v1", "-uall"]),
            status_before
        );
    }

    #[cfg(unix)] // builds paths from raw bytes with std::os::unix::ffi::OsStrExt
    #[test]
    fn native_document_paths_preserve_bytes_and_diagnostics_distinguish_names() {
        use std::os::unix::ffi::OsStrExt;
        for (raw, label) in [
            ("docs/déjà présent.md".as_bytes(), "docs/déjà présent.md"),
            (
                &b"docs/notes-\xff.md"[..],
                r"[escaped path bytes] docs/notes-\xff.md",
            ),
            (
                &b"docs/notes-\xfe.md"[..],
                r"[escaped path bytes] docs/notes-\xfe.md",
            ),
            (
                &b"docs/notes-\\xff.md"[..],
                r"[escaped path bytes] docs/notes-\\xff.md",
            ),
            (
                &b"docs/line\nbreak.md"[..],
                r"[escaped path bytes] docs/line\nbreak.md",
            ),
            (
                "docs/notes-\u{fffd}.md".as_bytes(),
                "docs/notes-\u{fffd}.md",
            ),
        ] {
            let path = native_document_path(raw).unwrap();
            assert_eq!(path.as_os_str().as_bytes(), raw);
            assert_eq!(document_path_label(&path), label);
        }
    }

    // The filesystem must accept arbitrary filename bytes. The local macOS
    // test volume rejects their creation with EILSEQ; pure conversions above
    // still run there. This fixture is qualified on Linux, not silently skipped.
    #[cfg(target_os = "linux")] // builds non-UTF-8 names from raw bytes with std::os::unix::ffi::OsStrExt
    #[tokio::test]
    async fn audit_handles_non_utf8_names_without_lossy_access_or_index_changes() {
        use std::os::unix::ffi::OsStrExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        let relative = Path::new(std::ffi::OsStr::from_bytes(b"docs/notes-\xff.md"));
        let path = root.join(relative);
        // A lossy conversion would alias this DIFFERENT, valid UTF-8 name.
        let unicode = root.join("docs/notes-\u{fffd}.md");
        std::fs::write(&path, "committed notes\n").unwrap();
        std::fs::write(&unicode, "separate Unicode document\n").unwrap();
        fixture_git(root, &["add", "docs"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        std::fs::write(&path, "staged human notes\n").unwrap();
        fixture_git(root, &["add", "docs"]);
        let working = "preexisting human architecture\n".repeat(600);
        std::fs::write(&path, &working).unwrap();
        let index_before = fixture_git(root, &["ls-files", "--stage", "-z"]);

        let before = snapshot_docs(root).await.unwrap();
        assert!(before.files.contains_key(relative));
        assert!(audit_rejections(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), working.as_bytes());
        assert_eq!(
            fixture_git(root, &["ls-files", "--stage", "-z"]),
            index_before
        );

        let fake_secret = "sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&path, fake_secret).unwrap();
        let fresh = root.join(std::ffi::OsStr::from_bytes(b"docs/notes-\xfe.md"));
        std::fs::write(&fresh, fake_secret).unwrap();
        let rejected = audit_rejections(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(rejected.len(), 2);
        for expected in [
            r"[escaped path bytes] docs/notes-\xfe.md",
            r"[escaped path bytes] docs/notes-\xff.md",
        ] {
            assert!(
                rejected.iter().any(|(label, _)| label == expected),
                "{rejected:?}"
            );
        }
        for file in [&path, &fresh] {
            assert_eq!(std::fs::read(file).unwrap(), fake_secret.as_bytes());
        }
        assert_eq!(
            std::fs::read_to_string(&unicode).unwrap(),
            "separate Unicode document\n"
        );
        assert_eq!(
            fixture_git(root, &["ls-files", "--stage", "-z"]),
            index_before
        );

        // Incomplete-audit diagnostics must identify the exact same path too.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len((MAX_AUDIT_BUFFER_BYTES + 1) as u64)
            .unwrap();
        // The document that cannot be read whole is told apart, and never
        // hides the known secret of the other one.
        let audit = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        let [error] = audit.incomplete.as_slice() else {
            panic!("{:?}", audit.incomplete);
        };
        assert!(error.contains(r"[escaped path bytes] docs/notes-\xff.md"));
        assert!(error.contains("content was not inspected and the file was preserved"));
        let [(rejected, _)] = audit.rejections.as_slice() else {
            panic!("{:?}", audit.rejections);
        };
        assert_eq!(rejected, r"[escaped path bytes] docs/notes-\xfe.md");
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            (MAX_AUDIT_BUFFER_BYTES + 1) as u64
        );
        assert_eq!(
            fixture_git(root, &["ls-files", "--stage", "-z"]),
            index_before
        );
    }

    #[tokio::test]
    async fn audit_checks_changed_content_without_erasing_concurrent_or_new_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        let old = root.join("docs/déjà présent.md");
        std::fs::write(&old, "human notes before the step\n").unwrap();
        let before = snapshot_docs(root).await.unwrap();
        let unsafe_content = "documentation example: sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&old, unsafe_content).unwrap();
        let fresh = root.join("docs/new report.md");
        std::fs::write(&fresh, unsafe_content).unwrap();
        let large = root.join("docs/architecture.md");
        let large_content = "ordinary architecture documentation, no credentials\n".repeat(1000);
        std::fs::write(&large, &large_content).unwrap();
        let rejected = audit_rejections(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(rejected.len(), 2, "large legitimate documents are accepted");
        assert!(rejected.iter().any(|(p, _)| p == "docs/déjà présent.md"));
        assert!(rejected.iter().any(|(p, _)| p == "docs/new report.md"));
        assert_eq!(std::fs::read_to_string(old).unwrap(), unsafe_content);
        assert_eq!(std::fs::read_to_string(fresh).unwrap(), unsafe_content);
        assert_eq!(std::fs::read_to_string(large).unwrap(), large_content);
        for (_, reason) in rejected {
            assert!(!reason.explain().contains("1234567890"));
        }
    }

    #[tokio::test]
    async fn audit_detects_new_docs_directory_and_preserves_binary_assets() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        let before = snapshot_docs(dir.path()).await.unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/diagram.bin"), [0xff, 0x80, 0]).unwrap();
        std::fs::write(
            dir.path().join("docs/new.md"),
            "sk-1234567890abcdefghijklmnopqrstuvwxyz",
        )
        .unwrap();
        let rejected = audit_rejections(dir.path(), &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0, "docs/new.md");
        assert_eq!(
            std::fs::read(dir.path().join("docs/diagram.bin")).unwrap(),
            [0xff, 0x80, 0]
        );
    }

    #[test]
    fn entropy_diagnostic_is_secret_free_and_unicode_safe() {
        let candidate = format!("{}x", "abcd1234".repeat(4));
        let content = format!("{candidate}éééééééééééééééééééé");
        let rejection = check_docs_write(&content, &SensitiveSubstrings::new()).unwrap_err();
        assert!(!rejection.explain().contains(&candidate));
    }

    #[tokio::test]
    async fn audit_excludes_ignored_files_but_checks_tracked_and_committed_changes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir_all(root.join("docs/generated")).unwrap();
        std::fs::write(root.join(".gitignore"), "docs/generated/\n").unwrap();
        let tracked = root.join("docs/generated/tracked.md");
        std::fs::write(&tracked, "original documentation").unwrap();
        fixture_git(root, &["add", "-f", "docs/generated/tracked.md"]);
        fixture_git(root, &["commit", "-q", "-m", "fixture"]);
        let before = snapshot_docs(root).await.unwrap();
        let fake_secret = "sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&tracked, fake_secret).unwrap();
        std::fs::write(root.join("docs/generated/ignored.md"), fake_secret).unwrap();
        std::fs::write(root.join("docs/committed.md"), fake_secret).unwrap();
        fixture_git(
            root,
            &[
                "add",
                "-f",
                "docs/committed.md",
                "docs/generated/tracked.md",
            ],
        );
        fixture_git(root, &["commit", "-q", "-m", "change during step"]);
        let rejected = audit_rejections(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(
            rejected
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["docs/committed.md", "docs/generated/tracked.md"]
        );
        let after = snapshot_docs(root).await.unwrap();
        assert_eq!(
            after.files.len(),
            2,
            "ignored untracked files must not be fingerprinted"
        );
        for path in [
            "docs/committed.md",
            "docs/generated/tracked.md",
            "docs/generated/ignored.md",
        ] {
            assert_eq!(
                std::fs::read_to_string(root.join(path)).unwrap(),
                fake_secret
            );
        }
        assert_eq!(
            fixture_git(root, &["show", ":docs/generated/tracked.md"]),
            fake_secret.as_bytes()
        );
    }

    #[tokio::test]
    async fn audit_bounds_changed_content_without_rejecting_unchanged_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        let path = root.join("docs/large.md");
        std::fs::write(&path, vec![b'a'; MAX_AUDIT_BUFFER_BYTES]).unwrap();
        let document = read_document(&path, true).unwrap().unwrap();
        assert_eq!(document.content.unwrap().len(), MAX_AUDIT_BUFFER_BYTES);
        let before = snapshot_docs(root).await.unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len((MAX_AUDIT_BUFFER_BYTES + 1) as u64).unwrap();
        let audit = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        let [error] = audit.incomplete.as_slice() else {
            panic!("{:?}", audit.incomplete);
        };
        assert!(error.contains("16 MiB audit buffer"));
        assert!(error.contains("file was preserved"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            (MAX_AUDIT_BUFFER_BYTES + 1) as u64
        );
        assert!(read_document(&path, true)
            .unwrap()
            .unwrap()
            .content
            .is_none());
        let unchanged = snapshot_docs(root).await.unwrap();
        assert!(
            audit_rejections(root, &SensitiveSubstrings::new(), &unchanged)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn audit_preserves_non_git_projects_without_inspecting_them() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        let before = snapshot_docs(dir.path()).await.unwrap();
        let path = dir.path().join("docs/new.md");
        let fake_secret = "sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&path, fake_secret).unwrap();
        assert!(
            audit_rejections(dir.path(), &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), fake_secret);
    }

    #[tokio::test]
    async fn audit_tolerates_documents_removed_during_the_step() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        let path = dir.path().join("docs/temporary.md");
        std::fs::write(&path, "ordinary document").unwrap();
        let before = snapshot_docs(dir.path()).await.unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(
            read_document(&path, false).unwrap().is_none(),
            "a listed file can vanish before open"
        );
        assert!(
            audit_rejections(dir.path(), &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!path.exists(), "audit never restores deleted files");
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[tokio::test]
    async fn audit_does_not_follow_document_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        fixture_git(dir.path(), &["init", "-q"]);
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        let path = outside.path().join("private.md");
        let original = "sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&path, original).unwrap();
        let before = snapshot_docs(dir.path()).await.unwrap();
        std::os::unix::fs::symlink(&path, dir.path().join("docs/link.md")).unwrap();
        assert!(
            audit_rejections(dir.path(), &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    /// The rejections of an audit alone.
    async fn audit_rejections(
        worktree: &Path,
        sensitive: &SensitiveSubstrings,
        before: &DocsSnapshot,
    ) -> Result<Vec<(String, SecretRejection)>, String> {
        audit_docs_writes(worktree, sensitive, before)
            .await
            .map(|audit| audit.rejections)
    }

    fn empty_subs() -> SensitiveSubstrings {
        SensitiveSubstrings::new()
    }

    // ─── Size cap ─────────────────────────────────────────────────────

    #[test]
    fn under_size_cap_passes() {
        let content = "A".repeat(MAX_ENTRY_BYTES - 1);
        assert!(check_docs_write(&content, &empty_subs()).is_ok());
    }

    #[test]
    fn at_size_cap_passes() {
        let content = "A".repeat(MAX_ENTRY_BYTES);
        assert!(check_docs_write(&content, &empty_subs()).is_ok());
    }

    #[test]
    fn over_size_cap_rejects() {
        let content = "A".repeat(MAX_ENTRY_BYTES + 1);
        let result = check_docs_write(&content, &empty_subs());
        assert!(matches!(result, Err(SecretRejection::TooLarge { .. })));
    }

    // ─── Denylist patterns ────────────────────────────────────────────

    #[test]
    fn rejects_anthropic_style_sk_token() {
        let content = "API key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn rejects_github_personal_access_token() {
        let content = "Token: ghp_abcdefghij1234567890ABCDEFGHIJKLMNOPQR";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn rejects_aws_access_key() {
        let content = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn rejects_pem_private_key_header() {
        let content = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA...\n-----END OPENSSH PRIVATE KEY-----";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn rejects_jwt_shape() {
        let content = "Auth: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTYifQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn rejects_atlassian_token() {
        let content = "JIRA_API_TOKEN=ATATT3xFfGF0vYP6UlRp1OlyjoMRdzPOFx_DT8J974jJsLCiOOQNlDOy45fwxxDLJ8dAHL6VTsbeNq1jN_st6Y3";
        let result = check_docs_write(content, &empty_subs());
        // Hits both ATATT and entropy run; either rejection is acceptable.
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
                | Err(SecretRejection::HighEntropyRun { .. })
        ));
    }

    #[test]
    fn rejects_password_assignment() {
        let content = "config has password=\"verysecretpassword\"";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::DenylistPattern { .. })
        ));
    }

    #[test]
    fn passes_clean_documentation_without_secrets() {
        let content = "BrandContext lives at application/src/Services/Brand/BrandContext.php.\n\
            It resolves brand from the request hostname.";
        assert!(check_docs_write(content, &empty_subs()).is_ok());
    }

    #[test]
    fn passes_when_password_keyword_is_part_of_prose() {
        // No `password=value` shape — just discussion.
        let content =
            "The login flow validates a password against bcrypt hashes stored in the users table.";
        assert!(check_docs_write(content, &empty_subs()).is_ok());
    }

    // ─── High-entropy detector ────────────────────────────────────────

    #[test]
    fn rejects_high_entropy_random_string() {
        // 32+ alphanumeric chars with healthy digit/letter mix.
        let content = "Saw this in the logs: aB3xZ9pQ7mK2vN4tL5fH6gJ8wR1sY0uT9oI2";
        let result = check_docs_write(content, &empty_subs());
        assert!(matches!(
            result,
            Err(SecretRejection::HighEntropyRun { .. })
        ));
    }

    #[test]
    fn passes_long_path_or_kebab_identifier() {
        // No digits in long stretch → not a token shape.
        let content = "See application/src/Services/Brand/BrandContextResolverFactoryAdapterImpl.php for details.";
        assert!(check_docs_write(content, &empty_subs()).is_ok());
    }

    #[test]
    fn passes_short_token_under_threshold() {
        let content = "Run: docker exec abc123def456";
        // 12 chars — under HIGH_ENTROPY_MIN_LEN, no other rule trips.
        assert!(check_docs_write(content, &empty_subs()).is_ok());
    }

    // ─── Sensitive file substring match ──────────────────────────────

    #[test]
    fn rejects_when_content_overlaps_env_value() {
        let mut subs = SensitiveSubstrings::new();
        subs.add("supersecretdbpassword123", ".env");
        let content = "DB connection uses supersecretdbpassword123 as the password.";
        let result = check_docs_write(content, &subs);
        match result {
            Err(SecretRejection::SensitiveFileSubstring { source, .. }) => {
                assert_eq!(source, ".env");
            }
            other => panic!("expected SensitiveFileSubstring, got {:?}", other),
        }
    }

    #[test]
    fn passes_when_substring_too_short_to_match() {
        let mut subs = SensitiveSubstrings::new();
        // < BLOOM_MIN_SUBSTRING_LEN (12) — should be silently dropped on add().
        subs.add("short", ".env");
        assert_eq!(subs.len(), 0, "short substrings must not be tracked");
        let content = "The string short appears here innocently.";
        assert!(check_docs_write(content, &subs).is_ok());
    }

    // ─── Detector ordering ───────────────────────────────────────────

    #[test]
    fn detectors_run_in_order_size_first() {
        let mut subs = SensitiveSubstrings::new();
        subs.add("AKIAIOSFODNN7EXAMPLE", ".env");
        // Construct content that's both too large AND has a denylist match.
        // Size cap should fire first (cheaper check).
        let content = format!("AKIAIOSFODNN7EXAMPLE\n{}", "A".repeat(MAX_ENTRY_BYTES));
        let result = check_docs_write(&content, &subs);
        assert!(matches!(result, Err(SecretRejection::TooLarge { .. })));
    }

    #[test]
    fn explain_returns_human_message() {
        let r = SecretRejection::DenylistPattern {
            pattern: "AKIA (AWS)",
            line: 1,
        };
        assert!(r.explain().contains("AKIA"));
        let r = SecretRejection::TooLarge { bytes: 9999 };
        assert!(r.explain().contains("9999"));
    }

    // ─── scan_sensitive_files ─────────────────────────────────────────

    #[test]
    fn scan_picks_up_env_values() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join(".env"),
            "DB_PASSWORD=actuallySecretValue123\nAPI_KEY=anothersecretkey4567\n# COMMENT=ignored\n",
        )
        .unwrap();
        let subs = scan_sensitive_files(tmp.path());
        assert!(
            subs.len() >= 2,
            "expected at least 2 substrings, got {}",
            subs.len()
        );
        // Substring should be matchable now.
        let content = "Note: this contains actuallySecretValue123 by accident.";
        assert!(matches!(
            check_docs_write(content, &subs),
            Err(SecretRejection::SensitiveFileSubstring { .. })
        ));
    }

    #[test]
    fn scan_ignores_node_modules_and_target() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("node_modules")).unwrap();
        std::fs::write(
            tmp.path().join("node_modules/.env"),
            "FAKE_KEY=this_should_be_ignored_long_enough",
        )
        .unwrap();
        let subs = scan_sensitive_files(tmp.path());
        assert_eq!(subs.len(), 0, "node_modules/.env must not be scanned");
    }

    #[test]
    fn scan_picks_up_pem_blocks() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("server.key"),
            "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwgg\n-----END PRIVATE KEY-----",
        ).unwrap();
        let subs = scan_sensitive_files(tmp.path());
        assert!(
            !subs.is_empty(),
            "expected to capture at least one pem line"
        );
    }

    // ─── KT-1796 — identifiers, runs already there, line numbers ─────

    /// 40 base64 characters, 14 of them digits.
    const BASE64_TOKEN: &str = "q8Z3vL0xT5mR2nB7kW9cY4hJ1pF6dS3gA8eU0iO2";
    /// 43 base64 characters with a `/`, as `openssl rand -base64 32` gives
    /// before its padding.
    const SLASH_TOKEN: &str = "Zk3xT9qLm2Vw8rYpB1nC/4hJ7dF0sG5aE6uKoI2tXbQ";
    /// 40 hexadecimal characters (a SHA-1).
    const HEX_TOKEN: &str = "3f786850e387550fdab836ed7e6dc881de23001b";

    #[test]
    fn a_rejection_names_its_line_without_the_content() {
        let content = format!("# Title\n\nIntro.\n\nValue {BASE64_TOKEN}\n");
        let explained = check_docs_write(&content, &SensitiveSubstrings::new())
            .unwrap_err()
            .explain();
        assert!(explained.contains("line 5"), "{explained}");
        assert!(!explained.contains(BASE64_TOKEN), "{explained}");

        let content = "one\ntwo\nkey sk-1234567890abcdefghijklmnopqrstuvwxyz\n";
        let explained = check_docs_content(content, &SensitiveSubstrings::new())
            .unwrap_err()
            .explain();
        assert!(explained.contains("line 3"), "{explained}");

        let mut subs = SensitiveSubstrings::new();
        subs.add("supersecretdbpassword123", ".env");
        let explained = check_docs_content("a\nuses supersecretdbpassword123\n", &subs)
            .unwrap_err()
            .explain();
        assert!(explained.contains("line 2"), "{explained}");
        assert!(
            !explained.contains("supersecretdbpassword123"),
            "{explained}"
        );
    }

    /// A document with one line of prose, then `lines`, changed by a step.
    async fn audit_of(lines: &[&str], sensitive: &SensitiveSubstrings) -> DocsAudit {
        let content = format!("# Notes\n{}\n", lines.join("\n"));
        audit_of_bytes(content.as_bytes(), sensitive).await
    }

    /// `audit_of`, for a document that is not plain UTF-8 text.
    async fn audit_of_bytes(content: &[u8], sensitive: &SensitiveSubstrings) -> DocsAudit {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/notes.md"), "# Notes\n").unwrap();
        let before = snapshot_docs(root).await.unwrap();
        std::fs::write(root.join("docs/notes.md"), content).unwrap();
        audit_docs_writes(root, sensitive, &before).await.unwrap()
    }

    /// KT-1796 — form cannot tell a token from an identifier: neither fails
    /// the step, both are reported with their line.
    #[tokio::test]
    async fn a_high_entropy_run_is_reported_and_fails_nothing() {
        let token = format!("Value {BASE64_TOKEN} here.");
        let lines = [
            "Debt TD-20260218-contexts-vs-serverdata-features is open.",
            "Prose only.",
            token.as_str(),
            "Commit 3f786850e387550fdab836ed7e6dc881de23001b fixed it.",
        ];
        let audit = audit_of(&lines, &SensitiveSubstrings::new()).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2, 4, 5])]
        );
    }

    #[tokio::test]
    async fn a_line_with_two_runs_is_reported_once() {
        let line = format!("{BASE64_TOKEN} then {HEX_TOKEN}");
        let audit = audit_of(&[line.as_str()], &SensitiveSubstrings::new()).await;
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2])]
        );
    }

    /// A possible secret earlier in the document never hides a known one.
    #[tokio::test]
    async fn a_known_secret_after_a_possible_one_still_fails_the_step() {
        let possible = format!("Value {BASE64_TOKEN} here.");
        let lines = [
            possible.as_str(),
            "key sk-1234567890abcdefghijklmnopqrstuvwxyz",
        ];
        let audit = audit_of(&lines, &SensitiveSubstrings::new()).await;
        assert!(
            matches!(
                audit.rejections.as_slice(),
                [(path, SecretRejection::DenylistPattern { line: 3, .. })] if path == "docs/notes.md"
            ),
            "{:?}",
            audit.rejections
        );

        let mut subs = SensitiveSubstrings::new();
        subs.add("supersecretdbpassword123", ".env");
        let lines = [possible.as_str(), "uses supersecretdbpassword123"];
        let audit = audit_of(&lines, &subs).await;
        assert!(
            matches!(
                audit.rejections.as_slice(),
                [(_, SecretRejection::SensitiveFileSubstring { line: 3, .. })]
            ),
            "{:?}",
            audit.rejections
        );
        // The possible one is still told; the rejected line is not told twice.
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2])]
        );
        assert_eq!(audit.inspected, vec!["docs/notes.md".to_string()]);

        // A rejected line can hold another run: it is told too.
        let known = format!("key sk-1234567890abcdefghijklmnopqrstuvwxyz and {BASE64_TOKEN}");
        let audit = audit_of(&[known.as_str()], &SensitiveSubstrings::new()).await;
        assert_eq!(audit.rejections.len(), 1);
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2])]
        );
    }

    /// What a document quotes from a sensitive file is rarely the whole
    /// value as the file spells it: a key cut out of a URL, one line of a
    /// key stored as a JSON string, a fragment, a value on a commented line
    /// or in a UTF-16 file.
    #[tokio::test]
    async fn a_part_of_a_sensitive_value_fails_the_step() {
        let long = format!("{BASE64_TOKEN}{HEX_TOKEN}{BASE64_TOKEN}");
        let utf16: Vec<u8> = format!("KEY={HEX_TOKEN}\n")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let cases: Vec<(&str, Vec<u8>, String)> = vec![
            (
                ".env",
                format!("SENTRY_DSN=https://{HEX_TOKEN}@sentry.example.com/42\n").into_bytes(),
                format!("Key {HEX_TOKEN} here."),
            ),
            (
                ".env",
                format!("HOOK_URL=https://ci.example.com/hooks/{BASE64_TOKEN}\n").into_bytes(),
                format!("Call https://other.example.org/x/{BASE64_TOKEN} now."),
            ),
            (
                "credentials.json",
                format!("{{\"private_key\": \"-----BEGIN X-----\\n{BASE64_TOKEN}\\n{HEX_TOKEN}\\n\"}}\n")
                    .into_bytes(),
                format!("Line {HEX_TOKEN} of the key."),
            ),
            (
                ".env",
                format!("TOKEN={long}\n").into_bytes(),
                format!("Starts with {} then stops.", &long[..45]),
            ),
            (
                ".env",
                format!("# LEGACY_API_SECRET_FOR_THE_OLD_GATEWAY={HEX_TOKEN}\n").into_bytes(),
                format!("Value {HEX_TOKEN} here."),
            ),
            (".env", utf16, format!("Value {HEX_TOKEN} here.")),
            // A base64 value with `/`, after a comment hid it from the
            // `KEY=VALUE` reading, glued to a name in the document.
            (
                ".env",
                format!("SESSION_SECRET={SLASH_TOKEN}= # rotate monthly\n").into_bytes(),
                format!("SESSION_SECRET_PRODUCTION_GATEWAY={SLASH_TOKEN}="),
            ),
            (
                "credentials.json",
                format!("{{\"id\":\"svc\",\"session_secret\":\"{SLASH_TOKEN}=\"}}\n").into_bytes(),
                format!("SECRET={SLASH_TOKEN}="),
            ),
            (
                ".env",
                format!("export K=\"{SLASH_TOKEN}\";\n").into_bytes(),
                format!("prefix_{SLASH_TOKEN}"),
            ),
        ];
        for (index, (name, body, line)) in cases.into_iter().enumerate() {
            let secrets = tempfile::tempdir().unwrap();
            std::fs::write(secrets.path().join(name), &body).unwrap();
            let subs = scan_sensitive_files(secrets.path());
            let audit = audit_of(&[line.as_str()], &subs).await;
            assert!(
                matches!(
                    audit.rejections.as_slice(),
                    [(_, SecretRejection::SensitiveFileSubstring { line: 2, .. })]
                ),
                "case {index} ({name}): {:?}",
                audit.rejections
            );
        }
    }

    /// A document glues what it quotes to a name (`hook_…`, `…-prod`,
    /// Markdown italics) where the file had a `/` or a quote: the run is
    /// no longer the file's, the 32 characters they share still are.
    #[tokio::test]
    async fn a_sensitive_value_glued_to_other_characters_fails_the_step() {
        let long = format!("{BASE64_TOKEN}{HEX_TOKEN}{BASE64_TOKEN}");
        let hook = format!("HOOK_URL=https://ci.example.com/hooks/{HEX_TOKEN}\n");
        let split = "aaaa1111bbbb2222=cccc3333dddd4444eeee";
        let cases: Vec<(&str, String, String)> = vec![
            (".env", hook.clone(), format!("hook_{HEX_TOKEN}")),
            (".env", hook.clone(), format!("In _{HEX_TOKEN}_ italics.")),
            (".env", hook.clone(), format!("id-{HEX_TOKEN}")),
            (".env", hook.clone(), format!("{HEX_TOKEN}-prod")),
            (".env", hook.clone(), format!("x{HEX_TOKEN}")),
            (".env", hook, format!("KEY={HEX_TOKEN}_v2")),
            (
                ".env",
                format!("GIT_URL=https://{HEX_TOKEN}@github.com/org/repo.git\n"),
                format!("tok-{HEX_TOKEN}"),
            ),
            (
                "credentials.json",
                format!("{{\n  \"conn\": \"{split}\",\n  \"id\": \"svc\"\n}}\n"),
                format!("conn_{split}"),
            ),
            (
                ".env",
                format!("T={long}\n"),
                format!("tok_{}", &long[..45]),
            ),
        ];
        for (index, (name, body, line)) in cases.into_iter().enumerate() {
            let secrets = tempfile::tempdir().unwrap();
            std::fs::write(secrets.path().join(name), &body).unwrap();
            let subs = scan_sensitive_files(secrets.path());
            let audit = audit_of(&[line.as_str()], &subs).await;
            assert!(
                matches!(
                    audit.rejections.as_slice(),
                    [(_, SecretRejection::SensitiveFileSubstring { line: 2, .. })]
                ),
                "case {index} ({name}): {:?}",
                audit.rejections
            );
        }
    }

    /// What a sensitive file holds is known, whatever its share of digits
    /// and whatever the document glues it to or cuts from it.
    #[tokio::test]
    async fn a_run_shared_with_a_sensitive_file_fails_the_step_whatever_its_digits() {
        // 42 characters, 4 digits: under the share a possible secret needs.
        let low = "kQwErTyUiOpAsDfGhJkLzXcVbNmQwErTyU1p2A3s4D";
        // 74 characters, 10 digits.
        let long = "mNbVcXzLkJhGfDsApOiUyTrEwQmNbVcXzLkJhGfDsApOiUyTrEwQmN0b1V2c3X4z5L6k7J8h9G";
        // 36 characters, 5 digits.
        let short = "hGfDsApOiUyTrEwQzXcVbNmLkJh1G2f3D4s5";
        let cases: Vec<(&str, String, String)> = vec![
            (
                "credentials.json",
                format!("{{\n  \"client_secret\": \"{low}\",\n  \"id\": \"svc\"\n}}\n"),
                format!("Secret {low} here."),
            ),
            (
                ".env",
                format!("APP_SIGNING={low} # rotate yearly\n"),
                format!("Secret {low} here."),
            ),
            (
                ".env",
                format!("APP_SIGNING={low}aBc==\n"),
                format!("Secret {low} here."),
            ),
            (
                ".env",
                format!("API_VALUE={long}\n"),
                format!("configurationprefixletters_{}", &long[..40]),
            ),
            (
                ".env",
                format!("API_VALUE={short}-production-environment-eu\n"),
                format!("id_9912345_{short}-production-env"),
            ),
            // Its digits at the end, past the first 32 characters.
            (
                ".env",
                format!("K={}12345678\n", &low[..32]),
                format!("x_{}1234", &low[..32]),
            ),
            // `+` as a JSON serialiser escapes it.
            (
                "credentials.json",
                format!("{{\"key\":\"{}\\u002b{}\"}}\n", &low[..20], &low[20..]),
                format!("key: {}+{}", &low[..20], &low[20..]),
            ),
        ];
        for (index, (name, body, line)) in cases.into_iter().enumerate() {
            let secrets = tempfile::tempdir().unwrap();
            std::fs::write(secrets.path().join(name), &body).unwrap();
            let subs = scan_sensitive_files(secrets.path());
            let audit = audit_of(&[line.as_str()], &subs).await;
            assert!(
                matches!(
                    audit.rejections.as_slice(),
                    [(_, SecretRejection::SensitiveFileSubstring { line: 2, .. })]
                ),
                "case {index} ({name}): {:?}",
                audit.rejections
            );
        }
    }

    /// What a sensitive file and a document share by convention is no
    /// secret: a rule, a variable name, a path without a digit.
    #[tokio::test]
    async fn the_ordinary_text_of_a_sensitive_file_fails_nothing() {
        let rule = "-".repeat(40);
        let bar = "=".repeat(40);
        let name = "REACT_APP_FIREBASE_MESSAGING_SENDER_ID";
        let path = "/srv/app/config/production/secrets/README";
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(
            secrets.path().join(".env"),
            format!("# {rule}\n#{bar}\n{name}=42\n# see {path}\n"),
        )
        .unwrap();
        let subs = scan_sensitive_files(secrets.path());
        let table = format!("|{rule}|{rule}|");
        let named = format!("Set `{name}` first.");
        let cited = format!("Read {path} before.");
        let lines = [
            "Title",
            bar.as_str(),
            table.as_str(),
            named.as_str(),
            cited.as_str(),
        ];
        let audit = audit_of(&lines, &subs).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
    }

    /// A document that cannot be read whole leaves the audit incomplete,
    /// not blind: what the others hold is still told.
    #[tokio::test]
    async fn a_document_too_large_leaves_the_others_audited() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        let before = snapshot_docs(root).await.unwrap();
        std::fs::write(
            root.join("docs/a.md"),
            format!("Value {BASE64_TOKEN} here.\n"),
        )
        .unwrap();
        std::fs::write(root.join("docs/dump.md"), vec![b'a'; 17 * 1024 * 1024]).unwrap();
        std::fs::write(
            root.join("docs/z.md"),
            "key sk-1234567890abcdefghijklmnopqrstuvwxyz\n",
        )
        .unwrap();
        let audit = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(audit.incomplete.len(), 1, "{:?}", audit.incomplete);
        assert!(
            audit.incomplete[0].contains("docs/dump.md"),
            "{:?}",
            audit.incomplete
        );
        assert_eq!(
            audit.possible_secrets,
            vec![
                ("docs/a.md".to_string(), vec![1]),
                ("docs/z.md".to_string(), vec![1])
            ]
        );
        assert_eq!(audit.rejections.len(), 1);
    }

    /// Nothing is set aside as binary: a file full of control bytes, a
    /// UTF-16 document in another script or a pasted terminal capture is
    /// read like any other, for known secrets and for possible ones.
    #[tokio::test]
    async fn no_document_is_set_aside_as_binary() {
        let noise: Vec<u8> = (0..4096u32).map(|i| (i * 131 % 251) as u8).collect();
        let mut bytes = noise.clone();
        bytes.extend_from_slice(b"\nkey sk-1234567890abcdefghijklmnopqrstuvwxyz\n");
        let audit = audit_of_bytes(&bytes, &SensitiveSubstrings::new()).await;
        assert_eq!(audit.rejections.len(), 1);

        let mut blob = noise;
        blob.extend_from_slice(format!("\n/ID <{HEX_TOKEN}>\n").as_bytes());
        let russian: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(
                format!("\u{414}\u{43e}\u{43a}\u{443}\u{43c}\u{435}\u{43d}\u{442}.\n\u{43a}\u{43b}\u{44e}\u{447}: {BASE64_TOKEN}\n")
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        let capture = format!("id\u{8}: {BASE64_TOKEN}\nrotate soon\n").into_bytes();
        for (index, bytes) in [blob, russian, capture].iter().enumerate() {
            let audit = audit_of_bytes(bytes, &SensitiveSubstrings::new()).await;
            assert!(audit.rejections.is_empty(), "case {index}");
            assert_eq!(audit.possible_secrets.len(), 1, "case {index}");
        }
    }

    /// The pattern list stays fast on the largest document the audit reads.
    #[test]
    fn a_large_binary_document_is_searched_in_seconds() {
        // A quarter of the audit buffer: 5 s for the whole of it, measured
        // on a debug build.
        let bytes: Vec<u8> = (0..(MAX_AUDIT_BUFFER_BYTES / 4) as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let started = std::time::Instant::now();
        let text = document_text(&bytes);
        let _ = check_docs_content(&text, &SensitiveSubstrings::new());
        let _ = possible_secret_lines(&text);
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_secs(60), "{elapsed:?}");
    }

    /// Prose in an 8-bit encoding is text: accents are no reason to skip it.
    #[tokio::test]
    async fn a_document_in_a_legacy_encoding_is_read_as_text() {
        // Latin-1: one byte per accented letter, no UTF-8.
        let prose = "Pr\u{e9}sentation g\u{e9}n\u{e9}rale de la cl\u{e9} d\u{e9}ploy\u{e9}e \u{e0} l'\u{e9}t\u{e9}.\n";
        let latin1 = |text: &str| -> Vec<u8> { text.chars().map(|c| c as u8).collect() };
        let known = latin1(&format!(
            "{prose}cl\u{e9} : sk-1234567890abcdefghijklmnopqrstuvwxyz\n"
        ));
        let audit = audit_of_bytes(&known, &SensitiveSubstrings::new()).await;
        assert_eq!(audit.rejections.len(), 1);

        let possible = latin1(&format!("{prose}valeur {BASE64_TOKEN}\n"));
        let audit = audit_of_bytes(&possible, &SensitiveSubstrings::new()).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2])]
        );
    }

    /// Two places of a sensitive file can each hold a part of what the
    /// document quotes: what they hold together is judged, not each alone.
    #[tokio::test]
    async fn a_value_held_in_two_places_of_a_sensitive_file_fails_the_step() {
        let head = format!("aBcDeFg{}", "h".repeat(33));
        let secret = format!("{head}iJkLmN");
        let decoys: String = (0..70)
            .map(|index| format!("  \"k{index}\": \"x_{head}iJ\",\n"))
            .collect();
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(
            secrets.path().join("credentials.json"),
            format!("{{\n{decoys}  \"client_secret\": \"{secret}\"\n}}\n"),
        )
        .unwrap();
        let subs = scan_sensitive_files(secrets.path());
        for line in [
            format!("rotation note: x_{secret}"),
            format!("secret {secret}"),
        ] {
            let audit = audit_of(&[line.as_str()], &subs).await;
            assert_eq!(audit.rejections.len(), 1, "{line}");
        }

        let serial = format!("0123456789{}", "0".repeat(31));
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(
            secrets.path().join(".env"),
            format!("# serial: {serial}\n# api_secret: {}ab\n", &serial[1..]),
        )
        .unwrap();
        let subs = scan_sensitive_files(secrets.path());
        let line = format!("value {serial}ab");
        let audit = audit_of(&[line.as_str()], &subs).await;
        assert_eq!(audit.rejections.len(), 1);
    }

    /// A document that is not plain UTF-8 is still read: UTF-16 text, or
    /// text with a stray byte, hides nothing.
    #[tokio::test]
    async fn a_document_in_another_encoding_is_still_audited() {
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(secrets.path().join(".env"), format!("KEY={HEX_TOKEN}\n")).unwrap();
        let subs = scan_sensitive_files(secrets.path());
        let text = format!("# Notes\nValue {HEX_TOKEN} here.\n");
        let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let with_bom: Vec<u8> = [0xff, 0xfe].into_iter().chain(utf16.clone()).collect();
        let mut stray = text.clone().into_bytes();
        stray.insert(3, 0xff);
        let denied: Vec<u8> = "# Notes\nkey sk-1234567890abcdefghijklmnopqrstuvwxyz\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        for (index, bytes) in [utf16, with_bom, stray, denied].iter().enumerate() {
            let audit = audit_of_bytes(bytes, &subs).await;
            assert_eq!(audit.rejections.len(), 1, "case {index}");
            assert_eq!(
                audit.inspected,
                vec!["docs/notes.md".to_string()],
                "case {index}"
            );
        }
        let possible: Vec<u8> = format!("# Notes\nValue {BASE64_TOKEN} here.\n")
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let audit = audit_of_bytes(&possible, &subs).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2])]
        );
    }

    /// One pass over a document made of runs, however many it holds.
    #[test]
    fn the_lines_of_many_runs_are_counted_in_one_pass() {
        let content = format!("{HEX_TOKEN}\n").repeat(60_000);
        let started = std::time::Instant::now();
        let lines = possible_secret_lines(&content);
        assert_eq!(lines.len(), 60_000);
        assert_eq!(lines.last(), Some(&60_000));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    /// The public metadata of a credential file is not a secret.
    #[tokio::test]
    async fn the_public_fields_of_a_credential_file_fail_nothing() {
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(
            secrets.path().join("credentials.json"),
            "{\n  \"type\": \"service_account\",\n  \"project_id\": \"my-proj-123456\",\n  \"token_uri\": \"https://oauth2.googleapis.com/token\"\n}\n",
        )
        .unwrap();
        let subs = scan_sensitive_files(secrets.path());
        let lines = [
            "We call storage.googleapis.com from the worker of my-proj-123456.",
            "It is a service_account, see https://oauth2.googleapis.com/token.",
        ];
        let audit = audit_of(&lines, &subs).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
    }

    /// A name glued to a token does not hide it from the reader.
    #[tokio::test]
    async fn a_token_after_its_name_is_still_reported() {
        let lines = [
            "GATEWAY_API_SECRET_KEY=q8ZvL0xTmR2nBkW9cYhJ1pFdS3gAeU0iOabc",
            "https://hooks.example.com/services/q8ZvL0xTmR2nBkW9cYhJ1pFdS3gAeU0iOabc",
            "PLAIN_NAME_WITHOUT_ANY_VALUE=see-the-vault-for-this-one",
        ];
        let audit = audit_of(&lines, &SensitiveSubstrings::new()).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
        assert_eq!(
            audit.possible_secrets,
            vec![("docs/notes.md".to_string(), vec![2, 3])]
        );
    }

    #[test]
    fn a_rescan_adds_nothing_it_already_holds() {
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(secrets.path().join(".env"), format!("KEY={HEX_TOKEN}\n")).unwrap();
        let first = scan_sensitive_files(secrets.path());
        let mut again = scan_sensitive_files(secrets.path());
        let (entries, texts) = (again.entries.len(), again.texts.len());
        assert_eq!(entries, 1, "{:?}", again.entries);
        again.extend_from(&first);
        assert_eq!((again.entries.len(), again.texts.len()), (entries, texts));
    }

    /// A run no sensitive file holds only warns, sensitive files or not.
    #[tokio::test]
    async fn a_run_no_sensitive_file_holds_still_fails_nothing() {
        let secrets = tempfile::tempdir().unwrap();
        std::fs::write(secrets.path().join(".env"), format!("KEY={HEX_TOKEN}\n")).unwrap();
        let subs = scan_sensitive_files(secrets.path());
        let line = format!("Value {BASE64_TOKEN} here.");
        let audit = audit_of(&[line.as_str()], &subs).await;
        assert!(audit.rejections.is_empty(), "{:?}", audit.rejections);
        assert_eq!(audit.possible_secrets.len(), 1);
    }

    /// A value the scanner does not extract verbatim (JSON, YAML, a trailing
    /// comment, a file that is not UTF-8) is still a known secret.
    #[tokio::test]
    async fn a_run_of_a_sensitive_file_fails_the_step_whatever_its_syntax() {
        for (name, body) in [
            (
                ".env",
                format!("SESSION_SECRET={BASE64_TOKEN} # rotate monthly\n").into_bytes(),
            ),
            (
                "credentials.json",
                format!("{{\n  \"private_key_id\": \"{HEX_TOKEN}\",\n}}\n").into_bytes(),
            ),
            (
                "credentials.yml",
                format!("token: {BASE64_TOKEN}\n").into_bytes(),
            ),
            (
                ".env.local",
                [
                    b"\xff\xfe name\n".as_slice(),
                    format!("KEY={HEX_TOKEN}\n").as_bytes(),
                ]
                .concat(),
            ),
        ] {
            let secrets = tempfile::tempdir().unwrap();
            std::fs::write(secrets.path().join(name), &body).unwrap();
            let subs = scan_sensitive_files(secrets.path());
            let token = if body
                .windows(BASE64_TOKEN.len())
                .any(|w| w == BASE64_TOKEN.as_bytes())
            {
                BASE64_TOKEN
            } else {
                HEX_TOKEN
            };
            let line = format!("Value {token} here.");
            let audit = audit_of(&[line.as_str()], &subs).await;
            assert!(
                matches!(
                    audit.rejections.as_slice(),
                    [(_, SecretRejection::SensitiveFileSubstring { line: 2, .. })]
                ),
                "{name}: {:?}",
                audit.rejections
            );
        }
    }

    /// What a step removed or changed is named, so that what was known of
    /// it is replaced; an untouched document is not.
    #[tokio::test]
    async fn the_audit_names_every_document_the_step_changed_or_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fixture_git(root, &["init", "-q"]);
        std::fs::create_dir(root.join("docs")).unwrap();
        for name in ["kept.md", "changed.md", "removed.md"] {
            std::fs::write(root.join("docs").join(name), format!("{BASE64_TOKEN}\n")).unwrap();
        }
        fixture_git(root, &["add", "docs"]);
        let before = snapshot_docs(root).await.unwrap();
        std::fs::write(root.join("docs/changed.md"), "Clean now.\n").unwrap();
        std::fs::remove_file(root.join("docs/removed.md")).unwrap();

        let audit = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap();
        assert_eq!(audit.inspected, vec!["docs/changed.md", "docs/removed.md"]);
        assert_eq!(audit.possible_secrets, vec![]);
    }

    /// A value of a sensitive file is rejected whatever its form: the
    /// detector that only warns never takes it over.
    #[tokio::test]
    async fn a_sensitive_file_value_shaped_like_a_token_still_fails_the_step() {
        let mut subs = SensitiveSubstrings::new();
        subs.add(BASE64_TOKEN, ".env");
        let line = format!("Value {BASE64_TOKEN} here.");
        let audit = audit_of(&[line.as_str()], &subs).await;
        assert!(
            matches!(
                audit.rejections.as_slice(),
                [(_, SecretRejection::SensitiveFileSubstring { line: 2, .. })]
            ),
            "{:?}",
            audit.rejections
        );
    }

    /// A memory entry has no reader to warn.
    #[test]
    fn a_memory_entry_with_a_possible_secret_is_rejected() {
        for content in [
            format!("Value {BASE64_TOKEN} here."),
            "Debt TD-20260218-contexts-vs-serverdata-features is open.".to_string(),
        ] {
            assert!(
                matches!(
                    check_docs_write(&content, &empty_subs()),
                    Err(SecretRejection::HighEntropyRun { line: 1 })
                ),
                "{content}"
            );
        }
    }
}
