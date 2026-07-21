//! Shared secret-redaction utility (0.8.6 #57).
//!
//! Single source of truth for "this looks like a credential, hide it" —
//! used by `db::api_call_logs` (in-place redact of stored excerpts) AND
//! the upcoming 0.9.0 `learning_candidates` (refuse content that looks
//! secret-y to avoid persisting tokens in `docs/AGENTS.md`).
//!
//! Mirrors `frontend/src/lib/bug-report.ts::redactSecrets` with a
//! superset of patterns so Rust-side coverage is at least as wide as
//! the FE-side bug-report flow.
//!
//! Design: conservative. False positives = redacted real text (still
//! readable, just less precise). False negatives = leaked credentials.
//! We err on the side of hiding.

use std::sync::LazyLock;

use regex_lite::Regex;

/// One redaction pattern: source regex + replacement template. `$N`
/// back-references behave like `regex_lite::Regex::replace_all`.
struct Pattern {
    re: Regex,
    replacement: &'static str,
}

// Source list: (regex_src, replacement). Wrapped in a LazyLock so the
// regex compilation happens once on first use, not at every call.
// Invalid patterns are skipped (filtered) — the test suite verifies all
// listed patterns compile, so a runtime skip means a typo was introduced.
//
// Order matters: catch-all-line patterns (Authorization headers, JSON
// credential fields, connection strings) MUST match BEFORE the bare
// vendor prefixes so we collapse a whole header instead of only the
// token suffix.
const RAW_PATTERNS: &[(&str, &str)] = &[
    // ── Catch-all-line patterns (must run first) ───────────────────
    //
    // Authorization headers — case-insensitive header, scheme keyword,
    // value to whitespace. Captures Bearer / Basic / Token / Digest.
    (
        r"(?i)(authorization\s*:\s*)(bearer|basic|token|digest)\s+\S+",
        "$1$2 ***REDACTED***",
    ),
    // JSON-encoded credentials. `"password": "..."`, `"token": "..."`,
    // `"api_key"`, `"apiKey"`, `"secret"`, `"access_token"`, `"refresh_token"`,
    // `"client_secret"`, `"private_key"`. Ported from FE bug-report.ts
    // (one entry there) + expanded.
    (
        r#"("(?:password|token|api_key|apiKey|secret|access_token|refresh_token|client_secret|private_key)"\s*:\s*")[^"]+(")"#,
        "$1***REDACTED***$2",
    ),
    // Connection strings with embedded credentials: scheme://user:pwd@host.
    // Covers postgres, mongodb, mysql, redis, amqp. Redacts the pwd only,
    // keeping the host visible (useful for debugging).
    (
        r"(?i)\b(postgres|postgresql|mongodb(?:\+srv)?|mysql|redis|amqp|amqps)://([^:@\s]+):([^@\s]+)@",
        "$1://$2:***REDACTED***@",
    ),
    // Bearer / token tokens that appear bare on a line (logs without
    // explicit header). Same shape as FE but tightened to avoid common
    // false positives ("bearer-brand-name").
    (
        r"\b(Bearer|Token|Basic)\s+([A-Za-z0-9._\-+/=]{20,})",
        "$1 ***REDACTED***",
    ),

    // ── Vendor-prefixed bare tokens ────────────────────────────────
    //
    // OpenAI / Anthropic sk-* keys (live + project + service variants).
    (r"\bsk-[A-Za-z0-9_-]{20,}\b", "sk-***REDACTED***"),
    // Anthropic Admin keys (p8e-* prefix).
    (r"\bp8e-[A-Za-z0-9_-]{8,}\b", "p8e-***REDACTED***"),
    // Google API keys.
    (r"\bAIza[0-9A-Za-z_-]{30,}\b", "AIza***REDACTED***"),
    // GitHub personal / fine-grained / app / refresh / server tokens.
    (r"\bgh[opsur]_[A-Za-z0-9_]{30,}\b", "gh*_***REDACTED***"),
    // Slack tokens (bot, user, app, app-level, webhook).
    (r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b", "xox*-***REDACTED***"),
    // JWT (three dot-separated base64 segments). Conservative length
    // floor (≥20 on the header) so we don't redact arbitrary text.
    (
        r"\beyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
        "***REDACTED-JWT***",
    ),
    // AWS access keys.
    (r"\bAKIA[0-9A-Z]{16}\b", "AKIA***REDACTED***"),
    // Stripe live + restricted keys (rk_live_*, rk_test_*, pk_live_*, etc.).
    // Note `${1}_${2}_` (not `$1_$2_`): regex_lite parses `$1_` as a
    // reference to group named "1_" (nonexistent → empty). Braces are
    // mandatory whenever a reference is followed by an identifier char.
    (
        r"\b(rk|pk)_(live|test)_[A-Za-z0-9]{20,}\b",
        "${1}_${2}_***REDACTED***",
    ),
];

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    RAW_PATTERNS
        .iter()
        .filter_map(|(src, repl)| {
            Regex::new(src)
                .map(|re| Pattern { re, replacement: repl })
                .map_err(|e| {
                    tracing::warn!(pattern = %src, error = %e, "core::redact: invalid pattern skipped");
                })
                .ok()
        })
        .collect()
});

/// Apply `patterns` sequentially and count only matches whose expanded
/// replacement differs from the matched bytes. Counting marker substrings is
/// unsafe: a secret may itself contain `***REDACTED***`, making a marker delta
/// zero even though the output changed. Using `Captures::expand` mirrors the
/// exact back-reference semantics used by `replace_all` while preserving the
/// important invariant `count > 0` iff the returned text changed.
fn apply_patterns(input: &str, patterns: &[Pattern]) -> (String, usize) {
    let mut out = input.to_string();
    let mut count = 0usize;

    for pat in patterns {
        let changed = pat
            .re
            .captures_iter(&out)
            .filter(|captures| {
                let Some(matched) = captures.get(0) else {
                    return false;
                };
                let mut replacement = String::new();
                captures.expand(pat.replacement, &mut replacement);
                replacement != matched.as_str()
            })
            .count();

        if changed == 0 {
            continue;
        }
        out = pat.re.replace_all(&out, pat.replacement).to_string();
        count = count.saturating_add(changed);
    }

    if out == input {
        (out, 0)
    } else {
        // Defensive floor: future pattern changes must never let callers see
        // changed content with a zero behavioral signal.
        (out, count.max(1))
    }
}

/// Apply every pattern once. Idempotent: running this on its own output
/// returns the same string (the replacement tokens don't match any
/// pattern themselves — verified by `redact_is_idempotent` test).
///
/// UTF-8 safe: `regex_lite::Regex::replace_all` works on `&str` so all
/// boundary handling is delegated to the regex engine.
pub fn redact_secrets(input: &str) -> String {
    apply_patterns(input, PATTERNS.as_slice()).0
}

/// Boolean version: returns `true` if ANY pattern would match. Used by
/// future 0.9.0 `learning_candidates` to *refuse* persisting content
/// that smells secret rather than just hiding it after the fact.
///
/// Cheaper than `redact_secrets` because it stops at the first match.
pub fn looks_like_secret(input: &str) -> bool {
    PATTERNS.iter().any(|pat| pat.re.is_match(input))
}

// ── Secret-ASSIGNMENT patterns for audit artifacts (0.8.13 blocker) ─────
//
// `redact_secrets` above only catches vendor-prefixed (`sk-`, `AIza`…) or
// JSON-keyword (`"secret": "…"`) shapes. Audit findings leak a different
// shape: a bare `NAME=value` / `name: value` where the NAME is secret-ish
// (`APP_SECRET=61cc…`, `apikey=…`) and the value has NO vendor prefix — so
// none of the patterns above fire. We anchor on the secret-ish NAME: this
// masks the value while NEVER touching ordinary hex checksums (which are
// path-keyed, e.g. `"docs/x.md": "<hex>"`, with no secret NAME), so the
// audit's own `docs/checksums.json` and `content_hash` fields stay intact.
//
// Two shapes, applied in order. The NAME alternation is repeated in both
// (regex_lite has no subroutines). We mask ANY non-empty value — quoted (may
// contain spaces/specials) or bare — because a short PIN or a symbol-laden
// password is just as much a leak (Codex review P0#4); no length floor.
//   1. quoted:   NAME [:=] "…"  → keep the quotes, mask the inside.
//   2. unquoted: NAME [:=] token → mask the token up to the next whitespace
//      / quote / separator.
// Anchored on the secret NAME, so ordinary path-keyed hex checksums (no NAME)
// are never touched. Quoted MUST run before unquoted (the unquoted value class
// excludes quotes, so it can't consume a quoted value itself).
const ASSIGNMENT_NAME: &str =
    r"[a-z0-9_.-]*(?:secret|apikey|api[_-]?key|passwd|password|pwd|pin|passcode|passphrase|access[_-]?key|private[_-]?key|signing[_-]?key|encryption[_-]?key|hmac[_-]?key|client[_-]?secret|auth[_-]?token|access[_-]?token|refresh[_-]?token|bearer[_-]?token)";

static ASSIGNMENT: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let quoted = format!(r#"(?i)(\b{ASSIGNMENT_NAME}\b\s*[:=]\s*)(["'`])([^"'`]+)(["'`])"#);
    let unquoted = format!(r#"(?i)(\b{ASSIGNMENT_NAME}\b\s*[:=]\s*)([^\s"'`,;]+)"#);
    [
        (quoted, "$1$2***REDACTED***$4"),
        (unquoted, "$1***REDACTED***"),
    ]
    .into_iter()
    .filter_map(|(src, repl)| {
        Regex::new(&src)
            .map(|re| Pattern { re, replacement: repl })
            .map_err(|e| {
                tracing::warn!(pattern = %src, error = %e, "core::redact: invalid assignment pattern skipped");
            })
            .ok()
    })
    .collect()
});

/// Redact for a Kronn AUDIT ARTIFACT (`docs/tech-debt/TD-*.md`,
/// `docs/inconsistencies-*.md`, index, reconciliation report). Applies the
/// vendor/keyword pass ([`redact_secrets`]) PLUS the secret-assignment pass,
/// so bare `APP_SECRET=…` / `apikey: …` literals — the shape that actually
/// leaked into TDs — are masked. Returns the redacted text and the number of
/// match replacements that changed bytes. The count is derived from the
/// replacements themselves, never from marker deltas, so a secret containing
/// `***REDACTED***` cannot suppress the write. The behavioral invariant is
/// `count > 0` iff the returned text differs from the input. Callers must NEVER
/// log the matched value itself. Re-running on already-redacted text is a no-op
/// with count 0.
///
/// Anchored on the secret NAME, so ordinary hex checksums/digests are left
/// intact — verified by the `audit_artifact_*` tests.
pub fn redact_for_audit_artifact(input: &str) -> (String, usize) {
    let (out, vendor_count) = apply_patterns(input, PATTERNS.as_slice());
    let (out, assignment_count) = apply_patterns(&out, ASSIGNMENT.as_slice());
    let count = vendor_count.saturating_add(assignment_count);
    if out == input {
        (out, 0)
    } else {
        (out, count.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Per-pattern positive tests (each MUST match + redact) ─────────

    #[test]
    fn redacts_authorization_bearer_header() {
        let out = redact_secrets("Authorization: Bearer abc123def456ghi789jkl");
        assert!(out.contains("Bearer ***REDACTED***"), "got: {out}");
        assert!(!out.contains("abc123def456ghi789jkl"));
    }

    #[test]
    fn redacts_authorization_basic_header_case_insensitive() {
        let out = redact_secrets("authorization: basic dXNlcjpwYXNzd29yZA==");
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("dXNlcjpwYXNzd29yZA"));
    }

    #[test]
    fn redacts_json_password_field() {
        let out = redact_secrets(r#"{"username":"alice","password":"hunter2"}"#);
        assert!(out.contains(r#""password":"***REDACTED***""#));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn redacts_json_access_token_field() {
        let out = redact_secrets(r#"{"access_token":"abcdefghijklmnop","ttl":3600}"#);
        assert!(out.contains(r#""access_token":"***REDACTED***""#));
        assert!(!out.contains("abcdefghijklmnop"));
    }

    #[test]
    fn redacts_postgres_connection_string() {
        let out = redact_secrets("postgres://app:s3cretP4ss@db.internal:5432/kronn");
        assert!(out.contains("postgres://app:***REDACTED***@db.internal"));
        assert!(!out.contains("s3cretP4ss"));
    }

    #[test]
    fn redacts_mongodb_srv_connection_string() {
        let out = redact_secrets("mongodb+srv://reader:VeryS3cret@cluster0.mongodb.net/db");
        assert!(out.contains("***REDACTED***"));
        assert!(!out.contains("VeryS3cret"));
    }

    #[test]
    fn redacts_bare_bearer_in_log_line() {
        let out = redact_secrets("got token: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc.def");
        assert!(out.contains("Bearer ***REDACTED***"));
    }

    #[test]
    fn redacts_openai_sk_key() {
        let out = redact_secrets("env: OPENAI_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz12345");
        assert!(out.contains("sk-***REDACTED***"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz12345"));
    }

    // ── Audit-artifact redaction (0.8.13 blocker) ─────────────────────
    // The shape that actually leaked into TDs: bare `NAME=value` where the
    // NAME is secret-ish and the value has NO vendor prefix (so plain
    // `redact_secrets` misses it). Fake values only — never a real secret.

    #[test]
    fn audit_artifact_masks_bare_app_secret_assignment() {
        // `redact_secrets` alone must MISS this (proves why we need the pass)…
        let plain = "committed 32-hex APP_SECRET=61cc954cdeadbeef0123456789abcdef in .env.dist:7";
        assert!(
            redact_secrets(plain).contains("61cc954cdeadbeef0123456789abcdef"),
            "guard: redact_secrets is not expected to catch a bare NAME=value",
        );
        // …and redact_for_audit_artifact must CATCH it.
        let (out, n) = redact_for_audit_artifact(plain);
        assert!(!out.contains("61cc954cdeadbeef0123456789abcdef"), "value must be masked: {out}");
        assert!(out.contains("APP_SECRET=***REDACTED***"), "name kept, value masked: {out}");
        assert!(n >= 1, "redaction count must be reported, got {n}");
    }

    #[test]
    fn audit_artifact_masks_apikey_assignments_various_ops() {
        for line in [
            "apikey=Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "apiKey: \"Ab3xZ9Qw7Lm2Ns5Pt8Rv\"",
            "here_api_key = Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "?apikey=Ab3xZ9Qw7Lm2Ns5Pt8Rv&lang=fr",
        ] {
            let (out, n) = redact_for_audit_artifact(line);
            assert!(!out.contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"), "value must be masked in {line:?}: {out}");
            assert!(n >= 1, "{line:?} should redact");
        }
    }

    #[test]
    fn audit_artifact_masks_short_and_special_and_quoted_values() {
        // P0#4: no length floor, specials, and quoted values with spaces.
        for (line, leak) in [
            ("password=admin", "admin"),
            ("secret: p@$$w0rd!#%", "p@$$w0rd!#%"),
            ("secret=99", "99"),
            ("client_secret: \"my long secret phrase\"", "my long secret phrase"),
            ("access_token='xyz.123-ABC'", "xyz.123-ABC"),
            ("signing_key=short!", "short!"),
            ("encryption-key: `two words`", "two words"),
            ("PIN=7", "7"),
        ] {
            let (out, n) = redact_for_audit_artifact(line);
            assert!(!out.contains(leak), "value {leak:?} must be masked in {line:?}: {out}");
            assert!(n >= 1, "{line:?} should redact");
        }
    }

    #[test]
    fn audit_artifact_leaves_ordinary_checksums_intact() {
        // checksums.json / content_hash: hex keyed by a PATH, no secret NAME.
        // MUST NOT be masked (would corrupt the F27 baseline).
        let checksum = r#""docs/inconsistencies-security.md": "3f5a9c2b8e1d4f6a0c7b2e9d1a4f8c3e5b7d9f1a3c5e7b9d1f3a5c7e9b1d3f5a""#;
        let (out, n) = redact_for_audit_artifact(checksum);
        assert_eq!(out, checksum, "path-keyed hex checksum must stay intact");
        assert_eq!(n, 0, "no redaction on ordinary checksums");

        let content_hash = "content_hash: 3f5a9c2b8e1d4f6a0c7b2e9d1a4f8c3e";
        let (out2, n2) = redact_for_audit_artifact(content_hash);
        assert_eq!(out2, content_hash, "content_hash is not a secret name");
        assert_eq!(n2, 0);
    }

    #[test]
    fn audit_artifact_still_catches_vendor_keys() {
        let (out, n) = redact_for_audit_artifact("key AIzaSyABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 here");
        assert!(out.contains("AIza***REDACTED***"), "vendor pass still applies: {out}");
        assert!(n >= 1);
    }

    #[test]
    fn audit_artifact_is_idempotent_and_clean_text_untouched() {
        let (once, _) = redact_for_audit_artifact("APP_SECRET=61cc954cdeadbeef0123456789abcdef");
        let (twice, n2) = redact_for_audit_artifact(&once);
        assert_eq!(once, twice, "re-running must be a no-op");
        assert_eq!(n2, 0, "already-redacted text redacts nothing more");
        // Ordinary prose is left alone.
        let prose = "The deploy step runs chown -R on /var and rsync excludes node_modules.";
        let (out, n) = redact_for_audit_artifact(prose);
        assert_eq!(out, prose);
        assert_eq!(n, 0);
    }

    #[test]
    fn audit_artifact_marker_inside_secret_cannot_zero_the_change_signal() {
        for (input, leak) in [
            (
                "APP_SECRET=prefix***REDACTED***still-secret",
                "prefix***REDACTED***still-secret",
            ),
            (
                r#"{"password":"prefix***REDACTED***still-secret"}"#,
                "prefix***REDACTED***still-secret",
            ),
        ] {
            let (out, count) = redact_for_audit_artifact(input);
            assert_ne!(out, input, "secret-bearing input must change: {input}");
            assert!(!out.contains(leak), "secret literal must be removed: {out}");
            assert!(count > 0, "changed output must report a positive count");

            let (again, second_count) = redact_for_audit_artifact(&out);
            assert_eq!(again, out, "redaction must remain idempotent");
            assert_eq!(second_count, 0, "already-redacted output is a no-op");
        }
    }

    #[test]
    fn redacts_anthropic_admin_p8e() {
        let out = redact_secrets("p8e-1234567890abcdef");
        assert!(out.contains("p8e-***REDACTED***"));
    }

    #[test]
    fn redacts_google_api_key() {
        let out = redact_secrets("AIzaSyAbcdEfGhIjKlMnOpQrStUvWxYz1234567");
        assert!(out.contains("AIza***REDACTED***"));
    }

    #[test]
    fn redacts_github_personal_token() {
        let out = redact_secrets("token=ghp_abcdefghijklmnopqrstuvwxyz1234567890");
        assert!(out.contains("gh*_***REDACTED***"));
    }

    #[test]
    fn redacts_github_fine_grained_token() {
        // gho_ user OAuth, ghs_ server, ghu_ user, ghr_ refresh.
        for prefix in ["gho_", "ghs_", "ghu_", "ghr_"] {
            let raw = format!("{}abcdefghijklmnopqrstuvwxyz1234567890", prefix);
            let out = redact_secrets(&raw);
            assert!(out.contains("***REDACTED***"), "prefix {prefix} not redacted: {out}");
        }
    }

    #[test]
    fn redacts_slack_bot_and_user_tokens() {
        for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-"] {
            let raw = format!("Slack: {}123456789012-abcdefABCDEF", prefix);
            let out = redact_secrets(&raw);
            assert!(out.contains("xox*-***REDACTED***"), "prefix {prefix} leaked: {out}");
        }
    }

    #[test]
    fn redacts_jwt_three_segments() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.signaturepart";
        let out = redact_secrets(&format!("jwt={jwt}"));
        assert!(out.contains("***REDACTED-JWT***"));
        assert!(!out.contains(jwt));
    }

    #[test]
    fn redacts_aws_access_key_id() {
        let out = redact_secrets("AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE");
        assert!(out.contains("AKIA***REDACTED***"));
    }

    #[test]
    fn redacts_stripe_live_keys() {
        // Build the input at runtime so the literal "rk_live_<key>" never
        // appears contiguously in source. GitHub Push Protection matches the
        // rk_live_ prefix + length (24+) REGARDLESS of entropy, so even an
        // obviously-fake literal trips it — splitting into fragments that
        // only join in memory is the only reliable way to keep a real test.
        let key = format!("rk_{}_{}", "live", "FAKEKEYxxxxxxxxxxxxxxxxxxxx");
        let out = redact_secrets(&key);
        assert!(out.contains("rk_live_***REDACTED***"), "got: {out}");
    }

    #[test]
    fn redacts_stripe_pk_test_keys() {
        let out = redact_secrets("pk_test_AbCdEfGhIjKlMnOpQrStUvWxYz1234567");
        assert!(out.contains("pk_test_***REDACTED***"), "got: {out}");
    }

    // ── False-positive guards (these MUST NOT be redacted) ────────────

    #[test]
    fn does_not_redact_short_lookalike() {
        // 4 chars after sk- — well below the 20-char floor.
        let s = "sk-foo";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn does_not_redact_word_containing_bearer() {
        // "bearer-brand-name" is text, not an auth scheme. Note: the bare
        // Bearer pattern requires `\b` + 20+ chars after; "brand-name" is
        // only 10. Stays untouched.
        let s = "the bearer-brand-name signals the contract";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn does_not_redact_aiza_short_or_unprefixed() {
        // "AIza" alone (no key body) must not be flagged.
        let s = "AIza is the prefix Google uses";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn does_not_redact_ghp_lookalike_too_short() {
        // ghp_ only has 5 chars after the underscore here — below the
        // 30-char floor.
        let s = "ghp_short";
        assert_eq!(redact_secrets(s), s);
    }

    #[test]
    fn does_not_redact_jwt_prefix_in_plain_word() {
        // "eyJ" alone is just text — needs the full 3-segment shape.
        let s = "eyJ is the base64 prefix of any JSON-encoded JWT header";
        assert_eq!(redact_secrets(s), s);
    }

    // ── UTF-8 + char boundary safety ──────────────────────────────────

    #[test]
    fn handles_utf8_around_match() {
        let s = "alice — Bearer abcdefghijklmnopqrstuv — done";
        let out = redact_secrets(s);
        assert!(out.contains("Bearer ***REDACTED***"));
        assert!(out.contains("alice — "));
        assert!(out.contains(" — done"));
    }

    #[test]
    fn handles_emoji_in_surrounding_text() {
        let s = "key: 🔑 sk-abcdefghijklmnopqrstuvwxyz12345";
        let out = redact_secrets(s);
        assert!(out.contains("🔑"));
        assert!(out.contains("sk-***REDACTED***"));
    }

    #[test]
    fn handles_multibyte_chars_at_boundary() {
        // Stress: ensure replacement doesn't slice mid-UTF-8.
        let s = "ééé Bearer abcdefghijklmnopqrstuvwxyz ééé";
        let out = redact_secrets(s);
        assert!(out.starts_with("ééé"));
        assert!(out.ends_with("ééé"));
        assert!(out.contains("***REDACTED***"));
    }

    // ── Multi-secret + composition ────────────────────────────────────

    #[test]
    fn redacts_multiple_secrets_in_one_string() {
        let s = "openai=sk-abcdefghijklmnopqrstuvwxyz12345 google=AIzaSy0123456789012345678901234567 done";
        let out = redact_secrets(s);
        assert!(out.contains("sk-***REDACTED***"));
        assert!(out.contains("AIza***REDACTED***"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(!out.contains("0123456789012345678901234567"));
    }

    #[test]
    fn redact_is_idempotent() {
        let s = "Authorization: Bearer abc123def456ghi789jklmn AIzaSy0123456789012345678901234567";
        let once = redact_secrets(s);
        let twice = redact_secrets(&once);
        assert_eq!(once, twice, "running redact twice changed the output");
    }

    #[test]
    fn redact_empty_string_returns_empty() {
        assert_eq!(redact_secrets(""), "");
    }

    // ── looks_like_secret() ───────────────────────────────────────────

    #[test]
    fn looks_like_secret_true_on_match() {
        assert!(looks_like_secret("here is sk-abcdefghijklmnopqrstuvwxyz12345"));
        assert!(looks_like_secret("authorization: bearer abcdefghijklmnopqrstuv"));
        assert!(looks_like_secret(r#"{"password": "anything"}"#));
        assert!(looks_like_secret("postgres://u:p@host/db"));
    }

    #[test]
    fn looks_like_secret_false_on_clean_text() {
        assert!(!looks_like_secret("plain prose with no credentials in sight"));
        assert!(!looks_like_secret("use the bearer-token-name convention"));
        assert!(!looks_like_secret("AIza prefix without a real key"));
        assert!(!looks_like_secret(""));
    }
}
