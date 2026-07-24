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
//! No side effects here — caller owns revert / commit / surface logic.
//!
//! ## Detection layers
//!
//! 1. **Hard size cap** : a memory fact is small (a path, a convention,
//!    a gotcha). > 8 KB usually = pasted log dump → reject.
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
/// (typically: revert via `git checkout`, log to run_state, surface in
/// run_detail UI).
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
            SecretRejection::HighEntropyRun { snippet } => {
                format!(
                    "write rejected: content has a 32+ char high-entropy run (likely a token): {:?}",
                    snippet
                )
            }
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
                let end = (s + 64).min(content.len());
                return Some(content[s..end].to_string());
            }
        }
    }
    if let Some(s) = start {
        let len = bytes.len() - s;
        if len >= HIGH_ENTROPY_MIN_LEN && looks_like_token(&content[s..]) {
            let end = (s + 64).min(content.len());
            return Some(content[s..end].to_string());
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

/// Audit any docs/ files modified during the previous agent step,
/// rejecting (and reverting via `git checkout`) those that fail the
/// secret-leak filter. Soft-mode : the step itself is not failed —
/// we just unwind the bad write and log loudly.
///
/// Returns the list of rejections (path → reason) so the caller can
/// surface them in run state / UI / telemetry.
///
/// Safe to call after EVERY step (no-op when nothing was modified or
/// the project has no `docs/` dir).
pub async fn audit_docs_writes(
    worktree: &Path,
    sensitive: &SensitiveSubstrings,
) -> Vec<(String, SecretRejection)> {
    let mut rejections = Vec::new();

    // Resolve the docs dir for this project (docs/ > doc/ > ai/ legacy).
    let docs_dir = crate::core::scanner::detect_docs_dir(worktree);
    let docs_rel = match docs_dir.strip_prefix(worktree) {
        Ok(p) => p.to_string_lossy().to_string(),
        Err(_) => return rejections, // Defensive — shouldn't happen.
    };

    // Use `git status --porcelain=v1 -uall` to find new + modified files.
    // Includes untracked (`??`) which the agent often creates fresh.
    let output = match crate::core::cmd::async_cmd("git")
        .args(["status", "--porcelain=v1", "-uall"])
        .current_dir(worktree)
        .output()
        .await
    {
        Ok(o) if o.status.success() => o,
        _ => return rejections,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        // Format: `XY <path>`. We only care about the path (last column).
        let path_str = match line.get(3..) {
            Some(p) => p.trim(),
            None => continue,
        };
        // Filter to docs_dir/ writes only.
        if !path_str.starts_with(&format!("{}/", docs_rel)) {
            continue;
        }
        // Skip the legacy index file rename and AGENTS.md root entry —
        // those are humans / curated audit, not agent free writes.
        if path_str.ends_with("/AGENTS.md") || path_str.ends_with("/index.md") {
            continue;
        }

        let abs = worktree.join(path_str);
        let content = match std::fs::read_to_string(&abs) {
            Ok(c) => c,
            Err(_) => continue, // Deleted or unreadable — ignore.
        };

        if let Err(reason) = check_docs_write(&content, sensitive) {
            tracing::warn!(
                target: "kronn::docs_write_filter",
                path = %path_str, reason = ?reason,
                "Rejected agent write to docs/ — reverting"
            );
            // Revert the write: `git checkout HEAD -- <path>` for tracked
            // files, or remove for untracked.
            let _ = revert_or_delete(worktree, path_str).await;
            rejections.push((path_str.to_string(), reason));
        }
    }

    rejections
}

async fn revert_or_delete(worktree: &Path, path: &str) {
    // Try `git checkout HEAD -- <path>` first (works for tracked files).
    let r = crate::core::cmd::async_cmd("git")
        .args(["checkout", "HEAD", "--", path])
        .current_dir(worktree)
        .output()
        .await;
    if let Ok(out) = r {
        if out.status.success() {
            return;
        }
    }
    // For untracked files: just delete.
    let abs = worktree.join(path);
    let _ = std::fs::remove_file(&abs);
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
