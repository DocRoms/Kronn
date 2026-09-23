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
//! 3. **High-entropy detector** : a 32+ char run of base64/hex with
//!    little semantic punctuation = likely a token. Mostly a backstop
//!    for vendor-specific tokens not yet in the denylist.
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
    /// Regex denylist matched. Field is the pattern that triggered.
    DenylistPattern { pattern: &'static str },
    /// 32+ char high-entropy run detected. Field is a short snippet of
    /// the offending content for the operator to identify the source.
    HighEntropyRun { snippet: String },
    /// Content matches a substring of a known sensitive worktree file.
    /// Field is the relative path of the source file.
    SensitiveFileSubstring { source: String },
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
            SecretRejection::DenylistPattern { pattern } => {
                format!(
                    "write rejected: content matches secret pattern {:?}",
                    pattern
                )
            }
            SecretRejection::HighEntropyRun { .. } =>
                "write rejected: content has a 32+ char high-entropy run (possible credential; content withheld)".into(),
            SecretRejection::SensitiveFileSubstring { source } => {
                format!(
                    "write rejected: content overlaps with substring from sensitive file {:?}",
                    source
                )
            }
        }
    }
}

/// Run all detectors in order of cheapness. Returns on first reject.
pub fn check_docs_write(
    content: &str,
    sensitive_substrings: &SensitiveSubstrings,
) -> Result<(), SecretRejection> {
    if content.len() > MAX_ENTRY_BYTES {
        return Err(SecretRejection::TooLarge {
            bytes: content.len(),
        });
    }
    check_docs_content(content, sensitive_substrings)
}

/// A project document is not a small memory entry. Its length alone is not
/// evidence of a credential leak; retain the actual content detectors.
fn check_docs_content(
    content: &str,
    sensitive_substrings: &SensitiveSubstrings,
) -> Result<(), SecretRejection> {
    if let Some(pattern) = match_denylist(content) {
        return Err(SecretRejection::DenylistPattern { pattern });
    }
    if let Some(snippet) = find_high_entropy_run(content) {
        return Err(SecretRejection::HighEntropyRun { snippet });
    }
    if let Some(source) = sensitive_substrings.matches(content) {
        return Err(SecretRejection::SensitiveFileSubstring { source });
    }
    Ok(())
}

/// Regex denylist match. Returns the matched pattern name on hit.
pub(crate) fn match_denylist(content: &str) -> Option<&'static str> {
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
    for (name, regex) in PATTERNS {
        if let Ok(re) = regex_lite::Regex::new(regex) {
            if re.is_match(content) {
                return Some(*name);
            }
        }
    }
    None
}

/// Detect a contiguous run of ≥ 32 chars of base64-or-hex characters
/// without semantic punctuation. Returns a 64-char snippet of the first
/// match for the operator to identify the source.
pub(crate) fn find_high_entropy_run(content: &str) -> Option<String> {
    let bytes = content.as_bytes();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        let is_token_char = b.is_ascii_alphanumeric()
            || b == b'_'
            || b == b'-'
            || b == b'/'
            || b == b'+'
            || b == b'=';
        if is_token_char {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            let len = i - s;
            if len >= HIGH_ENTROPY_MIN_LEN && looks_like_token(&content[s..i]) {
                return Some(content[s..i].chars().take(64).collect());
            }
        }
    }
    if let Some(s) = start {
        let len = bytes.len() - s;
        if len >= HIGH_ENTROPY_MIN_LEN && looks_like_token(&content[s..]) {
            return Some(content[s..].chars().take(64).collect());
        }
    }
    None
}

/// Heuristic: a "looks-like-token" run has a healthy mix of letters
/// AND digits AND no obvious English-word patterns. We require both
/// classes present and at least 30% of each.
fn looks_like_token(s: &str) -> bool {
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
    // Both classes must be present, and digits should be ≥ 15% to rule
    // out plain English / kebab-case identifiers.
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
}

impl SensitiveSubstrings {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a substring associated with a source path.
    pub fn add(&mut self, substring: impl Into<String>, source: impl Into<String>) {
        let s = substring.into();
        if s.len() < BLOOM_MIN_SUBSTRING_LEN {
            return; // ignore short tokens — too false-positive-prone
        }
        self.entries.push((s, source.into()));
    }

    /// Check if `content` contains any tracked sensitive substring.
    /// Returns the source path of the first match.
    pub fn matches(&self, content: &str) -> Option<String> {
        for (sub, src) in &self.entries {
            if content.contains(sub.as_str()) {
                return Some(src.clone());
            }
        }
        None
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
    let output = crate::core::cmd::sync_cmd("git")
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
        .current_dir(worktree)
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

/// Inspect only content changed since the pre-step snapshot. Never mutate the
/// worktree or index: another process may have authored any observed change.
/// The caller must fail the step on rejection or an incomplete audit.
pub async fn audit_docs_writes(
    worktree: &Path,
    sensitive: &SensitiveSubstrings,
    before: &DocsSnapshot,
) -> Result<Vec<(String, SecretRejection)>, String> {
    let mut rejections = Vec::new();
    let root = worktree.to_path_buf();
    let paths = tokio::task::spawn_blocking(move || document_paths(&root))
        .await
        .map_err(|e| format!("Document enumeration interrupted: {e}"))??;
    for path in paths {
        let absolute = worktree.join(&path);
        let document = tokio::task::spawn_blocking(move || read_document(&absolute, true))
            .await
            .map_err(|e| format!("Document read interrupted: {e}"))?
            .map_err(|e| {
                format!(
                    "Cannot audit changed document {}: {e}",
                    document_path_label(&path)
                )
            })?;
        let Some(document) = document else {
            continue;
        };
        if before.files.get(&path) == Some(&document.fingerprint) {
            continue;
        }
        let bytes = document.content.ok_or_else(|| format!(
            "Changed document {} exceeds the 16 MiB audit buffer; content was not inspected and the file was preserved", document_path_label(&path)
        ))?;
        // Binary assets are not textual memory/documentation.
        let Ok(content) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if let Err(reason) = check_docs_content(content, sensitive) {
            rejections.push((document_path_label(&path), reason));
        }
    }
    Ok(rejections)
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

#[cfg(test)]
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
        let rejected = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
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

    #[cfg(unix)]
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
    #[cfg(target_os = "linux")]
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
        assert!(
            audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(&path).unwrap(), working.as_bytes());
        assert_eq!(
            fixture_git(root, &["ls-files", "--stage", "-z"]),
            index_before
        );

        let fake_secret = "sk-1234567890abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&path, fake_secret).unwrap();
        let fresh = root.join(std::ffi::OsStr::from_bytes(b"docs/notes-\xfe.md"));
        std::fs::write(&fresh, fake_secret).unwrap();
        let rejected = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
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
        let error = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap_err();
        assert!(error.contains(r"[escaped path bytes] docs/notes-\xff.md"));
        assert!(error.contains("content was not inspected and the file was preserved"));
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
        let rejected = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
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
        let rejected = audit_docs_writes(dir.path(), &SensitiveSubstrings::new(), &before)
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
        let rejection = check_docs_content(&content, &SensitiveSubstrings::new()).unwrap_err();
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
        let rejected = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
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
        let error = audit_docs_writes(root, &SensitiveSubstrings::new(), &before)
            .await
            .unwrap_err();
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
            audit_docs_writes(root, &SensitiveSubstrings::new(), &unchanged)
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
            audit_docs_writes(dir.path(), &SensitiveSubstrings::new(), &before)
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
            audit_docs_writes(dir.path(), &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!path.exists(), "audit never restores deleted files");
    }

    #[cfg(unix)]
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
            audit_docs_writes(dir.path(), &SensitiveSubstrings::new(), &before)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
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
            Err(SecretRejection::SensitiveFileSubstring { source }) => {
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
}
