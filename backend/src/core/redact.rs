//! Shared secret-redaction utility (0.8.6 #57).
//!
//! Single source of truth for "this looks like a credential, hide it" —
//! used by `db::api_call_logs` (in-place redact of stored excerpts) AND
//! the upcoming 0.10.0 `learning_candidates` (refuse content that looks
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
    gate: Gate,
}

/// A cheap test that runs before a pattern's regex: most text holds no secret,
/// and `regex_lite` (a plain NFA simulation) is slow on it, so text that cannot
/// match is turned away without reaching the regex.
///
/// The rule that keeps this safe: **a gate must hold for every text its
/// pattern matches.** It may admit too much (the regex then decides), never too
/// little. Each gate below names the literal every match of its pattern must
/// contain; `regex_lite` folds case for ASCII only, so an ASCII needle also
/// covers the `(?i)` patterns. The `gates_*` tests check the rule against the
/// regexes themselves.
#[derive(Clone, Copy)]
enum Gate {
    /// The text contains one of these substrings, byte for byte.
    Contains(&'static [&'static str]),
    /// The text contains one of these lowercase ASCII substrings, ignoring
    /// ASCII case.
    ContainsAsciiCi(&'static [&'static str]),
    /// A secret-ish name, then optional whitespace, then `:` or `=` — the tail
    /// every secret-assignment pattern shares (see [`assignment_gate`]). A
    /// quoted-value pattern also needs its quote right after the `:`/`=`, past
    /// optional whitespace; `None` for the one that takes a bare value.
    Assignment(Option<u8>),
}

impl Gate {
    fn admits(self, text: &str) -> bool {
        match self {
            Gate::Contains(needles) => needles.iter().any(|needle| text.contains(needle)),
            Gate::ContainsAsciiCi(needles) => {
                let lowered = text.to_ascii_lowercase();
                needles.iter().any(|needle| lowered.contains(needle))
            }
            Gate::Assignment(quote) => assignment_gate(text, quote),
        }
    }
}

/// One pass over a text that turns it away for a whole list of patterns at
/// once, when none of their gates can admit it: a text of short identifiers and
/// paths (the bulk of what a resource holds) then costs a single scan instead of
/// one gate per pattern. Like a gate it may admit too much, never too little.
enum Prefilter {
    /// Admits everything: what a list falls back to when its gates are not of a
    /// shape this can summarise.
    Open,
    /// Three neighbouring bytes (ASCII case folded) that begin a literal of one
    /// of the list's gates, in a bitmap indexed by a hash of the triple. A text
    /// that holds a literal holds its first three bytes side by side, so a text
    /// with no such triple holds none of the literals; a hash collision only
    /// lets a text through that the gates then turn away.
    Triples(Vec<u64>),
    /// A `:` or `=`, which every secret-assignment pattern needs.
    Separator,
}

/// Slot of a triple of bytes in the [`Prefilter::Triples`] bitmap (16 bits).
///
/// `| 0x20` folds ASCII case in one step. It also folds a few non-letters
/// together (`-` and `\r`), which only ever lets more through, and the needle
/// side and the text side fold the same way.
fn triple_slot(first: u8, second: u8, third: u8) -> usize {
    (usize::from(first | 0x20) * 961 + usize::from(second | 0x20) * 31 + usize::from(third | 0x20))
        & 0xFFFF
}

impl Prefilter {
    fn literals(gates: impl Iterator<Item = Gate>) -> Self {
        let mut slots = vec![0u64; 1 << 10];
        for gate in gates {
            let (Gate::Contains(needles) | Gate::ContainsAsciiCi(needles)) = gate else {
                return Prefilter::Open;
            };
            for needle in needles {
                let &[first, second, third, ..] = needle.as_bytes() else {
                    return Prefilter::Open;
                };
                let slot = triple_slot(first, second, third);
                slots[slot >> 6] |= 1 << (slot & 63);
            }
        }
        Prefilter::Triples(slots)
    }

    fn admits(&self, text: &str) -> bool {
        match self {
            Prefilter::Open => true,
            Prefilter::Triples(slots) => {
                // A plain loop: this runs on every string of every resource, and
                // an iterator chain costs several times more in a debug build.
                let bytes = text.as_bytes();
                let mut start = 0;
                while start + 3 <= bytes.len() {
                    let slot = triple_slot(bytes[start], bytes[start + 1], bytes[start + 2]);
                    if (slots[slot >> 6] >> (slot & 63)) & 1 == 1 {
                        return true;
                    }
                    start += 1;
                }
                false
            }
            Prefilter::Separator => text.bytes().any(|byte| byte == b':' || byte == b'='),
        }
    }
}

// Source list: (regex_src, replacement, gate). Wrapped in a LazyLock so the
// regex compilation happens once on first use, not at every call.
// Invalid patterns are skipped (filtered) — the test suite verifies all
// listed patterns compile, so a runtime skip means a typo was introduced.
//
// Changing a pattern means re-checking its gate: the gate must still hold for
// everything the new pattern matches.
//
// Order matters: catch-all-line patterns (Authorization headers, JSON
// credential fields, connection strings) MUST match BEFORE the bare
// vendor prefixes so we collapse a whole header instead of only the
// token suffix.
const RAW_PATTERNS: &[(&str, &str, Gate)] = &[
    // ── Catch-all-line patterns (must run first) ───────────────────
    //
    // Authorization headers — case-insensitive header, scheme keyword,
    // value to whitespace. Captures Bearer / Basic / Token / Digest.
    (
        r"(?i)(authorization\s*:\s*)(bearer|basic|token|digest)\s+\S+",
        "$1$2 ***REDACTED***",
        Gate::ContainsAsciiCi(&["authorization"]),
    ),
    // JSON-encoded credentials. `"password": "..."`, `"token": "..."`,
    // `"api_key"`, `"apiKey"`, `"secret"`, `"access_token"`, `"refresh_token"`,
    // `"client_secret"`, `"private_key"`. Ported from FE bug-report.ts
    // (one entry there) + expanded.
    (
        r#"("(?:password|token|api_key|apiKey|secret|access_token|refresh_token|client_secret|private_key)"\s*:\s*")[^"]+(")"#,
        "$1***REDACTED***$2",
        Gate::Contains(&[
            "\"password\"",
            "\"token\"",
            "\"api_key\"",
            "\"apiKey\"",
            "\"secret\"",
            "\"access_token\"",
            "\"refresh_token\"",
            "\"client_secret\"",
            "\"private_key\"",
        ]),
    ),
    // Connection strings with embedded credentials: scheme://user:pwd@host.
    // Covers postgres, mongodb, mysql, redis, amqp. Redacts the pwd only,
    // keeping the host visible (useful for debugging).
    (
        r"(?i)\b(postgres|postgresql|mongodb(?:\+srv)?|mysql|redis|amqp|amqps)://([^:@\s]+):([^@\s]+)@",
        "$1://$2:***REDACTED***@",
        Gate::ContainsAsciiCi(&[
            "postgres://",
            "postgresql://",
            "mongodb://",
            "mongodb+srv://",
            "mysql://",
            "redis://",
            "amqp://",
            "amqps://",
        ]),
    ),
    // Bearer / token tokens that appear bare on a line (logs without
    // explicit header). Same shape as FE but tightened to avoid common
    // false positives ("bearer-brand-name").
    (
        r"\b(Bearer|Token|Basic)\s+([A-Za-z0-9._\-+/=]{20,})",
        "$1 ***REDACTED***",
        Gate::Contains(&["Bearer", "Token", "Basic"]),
    ),
    // ── Vendor-prefixed bare tokens ────────────────────────────────
    //
    // OpenAI / Anthropic sk-* keys (live + project + service variants).
    (
        r"\bsk-[A-Za-z0-9_-]{20,}\b",
        "sk-***REDACTED***",
        Gate::Contains(&["sk-"]),
    ),
    // Anthropic Admin keys (p8e-* prefix).
    (
        r"\bp8e-[A-Za-z0-9_-]{8,}\b",
        "p8e-***REDACTED***",
        Gate::Contains(&["p8e-"]),
    ),
    // Google API keys.
    (
        r"\bAIza[0-9A-Za-z_-]{30,}\b",
        "AIza***REDACTED***",
        Gate::Contains(&["AIza"]),
    ),
    // GitHub personal / fine-grained / app / refresh / server tokens.
    (
        r"\bgh[opsur]_[A-Za-z0-9_]{30,}\b",
        "gh*_***REDACTED***",
        Gate::Contains(&["ghp_", "gho_", "ghs_", "ghu_", "ghr_"]),
    ),
    // Slack tokens (bot, user, app, app-level, webhook).
    (
        r"\bxox[abprs]-[A-Za-z0-9-]{10,}\b",
        "xox*-***REDACTED***",
        Gate::Contains(&["xoxa-", "xoxb-", "xoxp-", "xoxr-", "xoxs-"]),
    ),
    // JWT (three dot-separated base64 segments). Conservative length
    // floor (≥20 on the header) so we don't redact arbitrary text.
    (
        r"\beyJ[A-Za-z0-9_-]{20,}\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
        "***REDACTED-JWT***",
        Gate::Contains(&["eyJ"]),
    ),
    // AWS access keys.
    (
        r"\bAKIA[0-9A-Z]{16}\b",
        "AKIA***REDACTED***",
        Gate::Contains(&["AKIA"]),
    ),
    // Stripe live + restricted keys (rk_live_*, rk_test_*, pk_live_*, etc.).
    // Note `${1}_${2}_` (not `$1_$2_`): regex_lite parses `$1_` as a
    // reference to group named "1_" (nonexistent → empty). Braces are
    // mandatory whenever a reference is followed by an identifier char.
    (
        r"\b(rk|pk)_(live|test)_[A-Za-z0-9]{20,}\b",
        "${1}_${2}_***REDACTED***",
        Gate::Contains(&["rk_live_", "rk_test_", "pk_live_", "pk_test_"]),
    ),
    // Stripe secret keys.
    (
        r"\bsk_(live|test)_[A-Za-z0-9]{16,}\b",
        "sk_${1}_***REDACTED***",
        Gate::Contains(&["sk_live_", "sk_test_"]),
    ),
    // GitLab personal tokens, npm and Hugging Face tokens.
    (
        r"\bglpat-[A-Za-z0-9_-]{16,}",
        "glpat-***REDACTED***",
        Gate::Contains(&["glpat-"]),
    ),
    (
        r"\bnpm_[A-Za-z0-9]{30,}\b",
        "npm_***REDACTED***",
        Gate::Contains(&["npm_"]),
    ),
    (
        r"\bhf_[A-Za-z0-9]{30,}\b",
        "hf_***REDACTED***",
        Gate::Contains(&["hf_"]),
    ),
    // Kronn's own bridge tokens (`core::bridge_token`).
    (
        r"\bkbt_[A-Za-z0-9]{16,}\b",
        "kbt_***REDACTED***",
        Gate::Contains(&["kbt_"]),
    ),
];

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    RAW_PATTERNS
        .iter()
        .filter_map(|(src, repl, gate)| {
            Regex::new(src)
                .map(|re| Pattern {
                    re,
                    replacement: repl,
                    gate: *gate,
                })
                .map_err(|e| {
                    tracing::warn!(pattern = %src, error = %e, "core::redact: invalid pattern skipped");
                })
                .ok()
        })
        .collect()
});

/// The prefilter of [`PATTERNS`]: built from every gate of [`RAW_PATTERNS`], a
/// superset of the patterns that compiled, so it can only admit more.
static VENDOR_PREFILTER: LazyLock<Prefilter> =
    LazyLock::new(|| Prefilter::literals(RAW_PATTERNS.iter().map(|(_, _, gate)| *gate)));

/// The prefilter of [`ASSIGNMENT`].
static ASSIGNMENT_PREFILTER: Prefilter = Prefilter::Separator;

/// Apply `patterns` sequentially and count only matches whose expanded
/// replacement differs from the matched bytes. Counting marker substrings is
/// unsafe: a secret may itself contain `***REDACTED***`, making a marker delta
/// zero even though the output changed. Using `Captures::expand` mirrors the
/// exact back-reference semantics used by `replace_all` while preserving the
/// important invariant `count > 0` iff the returned text changed.
fn apply_patterns(input: &str, patterns: &[Pattern], prefilter: &Prefilter) -> (String, usize) {
    let mut out = input.to_string();
    let mut count = 0usize;
    // The prefilter's verdict on `out` as it stands: asked again each time a
    // pattern rewrites the text, since the replacement can bring literals of its
    // own (`Bearer ***REDACTED***`).
    let mut open = prefilter.admits(&out);

    for pat in patterns {
        if !open {
            break;
        }
        // The gate is checked on the text as the previous patterns left it,
        // exactly what the regex is about to see.
        if !pat.gate.admits(&out) {
            continue;
        }
        // One scan: the same expansion `replace_all` would write, counted as it
        // goes. `regex_lite` matches with a plain NFA simulation, so a second
        // scan (one to count, one to replace) doubles the cost of every
        // pattern that does match.
        let mut next = String::with_capacity(out.len());
        let mut copied = 0usize;
        let mut changed = 0usize;
        for captures in pat.re.captures_iter(&out) {
            let Some(matched) = captures.get(0) else {
                continue;
            };
            next.push_str(&out[copied..matched.start()]);
            let written = next.len();
            captures.expand(pat.replacement, &mut next);
            if next[written..] != *matched.as_str() {
                changed += 1;
            }
            copied = matched.end();
        }

        if changed == 0 {
            continue;
        }
        next.push_str(&out[copied..]);
        out = next;
        count = count.saturating_add(changed);
        open = prefilter.admits(&out);
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
    apply_patterns(input, PATTERNS.as_slice(), &VENDOR_PREFILTER).0
}

/// Boolean version: returns `true` if ANY pattern would match. Used by
/// future 0.10.0 `learning_candidates` to *refuse* persisting content
/// that smells secret rather than just hiding it after the fact.
///
/// Cheaper than `redact_secrets` because it stops at the first match.
pub fn looks_like_secret(input: &str) -> bool {
    VENDOR_PREFILTER.admits(input)
        && PATTERNS
            .iter()
            .any(|pat| pat.gate.admits(input) && pat.re.is_match(input))
}

// ── Secret-ASSIGNMENT patterns for audit artifacts (0.9.0 blocker) ─────
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
//   1. quoted:   NAME [:=] "…"  → keep the quotes, mask the inside. Each
//      delimiter has its own pattern so a different quote inside the secret
//      cannot terminate the match early and leak the remaining suffix.
//   2. unquoted: NAME [:=] token → mask the token up to the next whitespace
//      / quote / separator.
// Anchored on the secret NAME, so ordinary path-keyed hex checksums (no NAME)
// are never touched. Quoted MUST run before unquoted (the unquoted value class
// excludes quotes, so it can't consume a quoted value itself).
const ASSIGNMENT_NAMES: &str = r"secret|apikey|api[_-]?key|passwd|password|pwd|pin|passcode|passphrase|access[_-]?key|private[_-]?key|signing[_-]?key|encryption[_-]?key|hmac[_-]?key|client[_-]?secret|token";

static ASSIGNMENT: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let name = format!("[a-z0-9_.-]*(?:{ASSIGNMENT_NAMES})");
    let quoted_dq = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)\"((?:\\\\.|[^\"\\\\])+)\"");
    let quoted_sq = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)'((?:\\\\.|[^'\\\\])+)'");
    let quoted_bt = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)`((?:\\\\.|[^`\\\\])+)`");
    let unquoted = format!(r#"(?i)(\b{name}\b\s*[:=]\s*)([^\s"'`,;]+)"#);
    [
        (quoted_dq, "$1\"***REDACTED***\"", Some(b'"')),
        (quoted_sq, "$1'***REDACTED***'", Some(b'\'')),
        (quoted_bt, "$1`***REDACTED***`", Some(b'`')),
        (unquoted, "$1***REDACTED***", None),
    ]
    .into_iter()
    .filter_map(|(src, repl, quote)| {
        Regex::new(&src)
            .map(|re| Pattern {
                re,
                replacement: repl,
                gate: Gate::Assignment(quote),
            })
            .map_err(|e| {
                tracing::warn!(pattern = %src, error = %e, "core::redact: invalid assignment pattern skipped");
            })
            .ok()
    })
    .collect()
});

/// Lowercase spellings of every name in [`ASSIGNMENT_NAMES`], with `[_-]?`
/// expanded to its three readings (`api[_-]?key` → `apikey`, `api_key`,
/// `api-key`). `None` when the list uses any syntax this expansion does not
/// know: [`assignment_gate`] then admits everything rather than guess.
static ASSIGNMENT_TAILS: LazyLock<Option<Vec<Vec<u8>>>> = LazyLock::new(|| {
    let mut tails = Vec::new();
    for name in ASSIGNMENT_NAMES.split('|') {
        let spellings = match name.split_once("[_-]?") {
            Some((head, tail)) => ["", "_", "-"]
                .map(|separator| format!("{head}{separator}{tail}"))
                .to_vec(),
            None => vec![name.to_string()],
        };
        for spelling in spellings {
            let plain = !spelling.is_empty()
                && spelling
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte == b'-');
            if !plain {
                return None;
            }
            tails.push(spelling.into_bytes());
        }
    }
    Some(tails)
});

/// Gate of the secret-assignment patterns. Each of them needs a name from
/// [`ASSIGNMENT_NAMES`], then `\b\s*[:=]`, so a match always contains a `:` or
/// `=` preceded, past optional whitespace, by one of those names (ASCII case
/// ignored, as `regex_lite` folds no more than that). The name is a *suffix* of
/// a longer word (`APP_SECRET`, `iris_token`) and always ends in a letter, so
/// the closing `\b` holds whenever `:`/`=` or a space follows — nothing more to
/// check. `\s` is ASCII whitespace; any byte up to a space over-approximates it.
///
/// A quoted-value pattern reads its opening `quote` right after `[:=]\s*`, so
/// the first byte past the `:`/`=` that is not up to a space must be that quote.
fn assignment_gate(text: &str, quote: Option<u8>) -> bool {
    let Some(tails) = ASSIGNMENT_TAILS.as_ref() else {
        return true;
    };
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        if *byte != b':' && *byte != b'=' {
            return false;
        }
        if let Some(quote) = quote {
            let opens = bytes[index + 1..].iter().find(|byte| **byte > b' ');
            if opens != Some(&quote) {
                return false;
            }
        }
        let before = &bytes[..index];
        let end = before
            .iter()
            .rposition(|byte| *byte > b' ')
            .map_or(0, |last| last + 1);
        let before = &before[..end];
        tails.iter().any(|tail| {
            before.len() >= tail.len()
                && before[before.len() - tail.len()..].eq_ignore_ascii_case(tail)
        })
    })
}

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
    let (out, vendor_count) = apply_patterns(input, PATTERNS.as_slice(), &VENDOR_PREFILTER);
    let (out, assignment_count) =
        apply_patterns(&out, ASSIGNMENT.as_slice(), &ASSIGNMENT_PREFILTER);
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

    // ── Audit-artifact redaction (0.9.0 blocker) ─────────────────────
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
        assert!(
            !out.contains("61cc954cdeadbeef0123456789abcdef"),
            "value must be masked: {out}"
        );
        assert!(
            out.contains("APP_SECRET=***REDACTED***"),
            "name kept, value masked: {out}"
        );
        assert!(n >= 1, "redaction count must be reported, got {n}");
    }

    #[test]
    fn audit_artifact_masks_bare_token_assignment() {
        // The shape that actually leaked in an iris-api TD: a bare `TOKEN=` /
        // `iris_token:` name with no auth/access/refresh/bearer qualifier and
        // no vendor prefix, so neither the old ASSIGNMENT_NAME alternation nor
        // `redact_secrets` caught it.
        for line in [
            "TOKEN=Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "iris_token: Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "session_token=\"Ab3xZ9Qw7Lm2Ns5Pt8Rv\"",
        ] {
            let (out, n) = redact_for_audit_artifact(line);
            assert!(
                !out.contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"),
                "token value must be masked in {line:?}: {out}"
            );
            assert!(n >= 1, "{line:?} should redact");
        }
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
            assert!(
                !out.contains("Ab3xZ9Qw7Lm2Ns5Pt8Rv"),
                "value must be masked in {line:?}: {out}"
            );
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
            (
                "client_secret: \"my long secret phrase\"",
                "my long secret phrase",
            ),
            ("access_token='xyz.123-ABC'", "xyz.123-ABC"),
            ("signing_key=short!", "short!"),
            ("encryption-key: `two words`", "two words"),
            ("PIN=7", "7"),
        ] {
            let (out, n) = redact_for_audit_artifact(line);
            assert!(
                !out.contains(leak),
                "value {leak:?} must be masked in {line:?}: {out}"
            );
            assert!(n >= 1, "{line:?} should redact");
        }
    }

    #[test]
    fn audit_artifact_mixed_quotes_cannot_leak_a_secret_suffix() {
        for (line, leak, expected) in [
            (
                r#"password="pa'ss""#,
                "pa'ss",
                r#"password="***REDACTED***""#,
            ),
            (
                r#"client_secret='pa"ss'"#,
                r#"pa"ss"#,
                "client_secret='***REDACTED***'",
            ),
            (
                "signing_key=`pa'\"ss`",
                "pa'\"ss",
                "signing_key=`***REDACTED***`",
            ),
            (
                r#"password="pa\"ss""#,
                r#"pa\"ss"#,
                r#"password="***REDACTED***""#,
            ),
        ] {
            let (out, n) = redact_for_audit_artifact(line);
            assert_eq!(
                out, expected,
                "mixed quote value must be fully masked: {line:?}"
            );
            assert!(
                !out.contains(leak),
                "secret suffix leaked from {line:?}: {out}"
            );
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
        let (out, n) =
            redact_for_audit_artifact("key AIzaSyABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 here");
        assert!(
            out.contains("AIza***REDACTED***"),
            "vendor pass still applies: {out}"
        );
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
            assert!(
                out.contains("***REDACTED***"),
                "prefix {prefix} not redacted: {out}"
            );
        }
    }

    #[test]
    fn redacts_slack_bot_and_user_tokens() {
        for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-"] {
            let raw = format!("Slack: {}123456789012-abcdefABCDEF", prefix);
            let out = redact_secrets(&raw);
            assert!(
                out.contains("xox*-***REDACTED***"),
                "prefix {prefix} leaked: {out}"
            );
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
        assert!(looks_like_secret(
            "here is sk-abcdefghijklmnopqrstuvwxyz12345"
        ));
        assert!(looks_like_secret(
            "authorization: bearer abcdefghijklmnopqrstuv"
        ));
        assert!(looks_like_secret(r#"{"password": "anything"}"#));
        assert!(looks_like_secret("postgres://u:p@host/db"));
    }

    #[test]
    fn looks_like_secret_false_on_clean_text() {
        assert!(!looks_like_secret(
            "plain prose with no credentials in sight"
        ));
        assert!(!looks_like_secret("use the bearer-token-name convention"));
        assert!(!looks_like_secret("AIza prefix without a real key"));
        assert!(!looks_like_secret(""));
    }

    // ── Gates: the fast path may never change what gets redacted ───────

    /// `apply_patterns` as it was before gates: every regex, every time.
    fn apply_patterns_ungated(input: &str, patterns: &[Pattern]) -> (String, usize) {
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
            (out, count.max(1))
        }
    }

    fn every_pattern() -> impl Iterator<Item = &'static Pattern> {
        PATTERNS.iter().chain(ASSIGNMENT.iter())
    }

    /// The prefilter that guards the list `pat` belongs to.
    fn prefilter_of(pat: &Pattern) -> &'static Prefilter {
        if ASSIGNMENT.iter().any(|item| std::ptr::eq(item, pat)) {
            &ASSIGNMENT_PREFILTER
        } else {
            &VENDOR_PREFILTER
        }
    }

    #[test]
    fn gates_are_built_for_every_pattern_and_the_name_list_is_understood() {
        assert_eq!(
            PATTERNS.len(),
            RAW_PATTERNS.len(),
            "a pattern failed to compile"
        );
        assert_eq!(ASSIGNMENT.len(), 4);
        assert!(
            matches!(&*VENDOR_PREFILTER, Prefilter::Triples(_)),
            "a gate literal shorter than three bytes turns the vendor prefilter off"
        );
        let tails = ASSIGNMENT_TAILS
            .as_ref()
            .expect("the assignment names use only syntax the gate expands");
        for expected in [
            "secret",
            "token",
            "password",
            "pin",
            "apikey",
            "api_key",
            "api-key",
            "hmac-key",
            "private_key",
            "signing-key",
            "encryption_key",
            "client_secret",
            "access-key",
        ] {
            assert!(
                tails.iter().any(|tail| tail == expected.as_bytes()),
                "{expected} missing from {tails:?}"
            );
        }
    }

    #[test]
    fn redacts_the_r24_vendor_tokens() {
        for (raw, secret) in [
            (
                "git clone https://oauth2:glpat-AbCdEfGhIjKlMnOpQrSt@gitlab.com/o/r.git",
                "AbCdEfGh",
            ),
            ("echo sk_live_abcdefghijklmnopqrstuvwx", "abcdefghijklmnop"),
            (
                "token npm_abcdefghijklmnopqrstuvwxyz0123456789",
                "abcdefghijklmnop",
            ),
            (
                "hf_abcdefghijklmnopqrstuvwxyz0123456789",
                "abcdefghijklmnop",
            ),
            (
                "KRONN kbt_0123456789abcdef0123456789abcdef",
                "0123456789abcdef",
            ),
        ] {
            let out = redact_secrets(raw);
            assert!(!out.contains(secret), "{raw:?} -> {out:?}");
            assert_eq!(redact_secrets(&out), out, "idempotent on {raw:?}");
        }
    }

    #[test]
    fn gates_admit_every_shape_their_pattern_matches() {
        for sample in [
            "Authorization: Bearer abc123def456ghi789jkl",
            "AUTHORIZATION :\tbasic dXNlcjpwYXNzd29yZA==",
            r#"{"username":"alice","password":"hunter2"}"#,
            r#"{"apiKey": "abcdefghijklmnop"}"#,
            "postgres://app:s3cretP4ss@db.internal:5432/kronn",
            "MongoDB+SRV://reader:VeryS3cret@cluster0.mongodb.net/db",
            "Bearer abcdefghijklmnopqrstuvwxyz",
            "sk-abcdefghijklmnopqrstuvwxyz12345",
            "p8e-1234567890abcdef",
            "AIzaSyAbcdEfGhIjKlMnOpQrStUvWxYz1234567",
            "ghp_abcdefghijklmnopqrstuvwxyz1234567890",
            "gho_abcdefghijklmnopqrstuvwxyz1234567890",
            "xoxb-1234567890-abcdefghij",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.abc.def",
            "AKIAABCDEFGHIJKLMNOP",
            "rk_live_abcdefghijklmnopqrstuv",
            "pk_test_abcdefghijklmnopqrstuv",
            "https://oauth2:glpat-AbCdEfGhIjKlMnOpQrSt@gitlab.com/o/r.git",
            "sk_live_abcdefghijklmnopqrstuvwx",
            "glpat-AbCdEfGhIjKlMnOpQrSt",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
            "hf_abcdefghijklmnopqrstuvwxyz0123456789",
            "kbt_0123456789abcdef0123456789abcdef",
            "APP_SECRET=61cc954cdeadbeef0123456789abcdef",
            "iris_token: Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "session_token=\"Ab3xZ9Qw7Lm2Ns5Pt8Rv\"",
            "apiKey : 'Ab3xZ9Qw7Lm2Ns5Pt8Rv'",
            "here_api-key\t=\u{b}Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "encryption-key: `two words`",
            "é_PIN=7",
            "HMAC_KEY\n=\nAb3x",
            "clientsecret=Ab3x",
        ] {
            let matching: Vec<_> = every_pattern()
                .filter(|pat| pat.re.is_match(sample))
                .collect();
            assert!(!matching.is_empty(), "{sample:?} should match a pattern");
            for pat in matching {
                assert!(
                    pat.gate.admits(sample),
                    "gate holds back a match of {} on {sample:?}",
                    pat.re.as_str()
                );
                assert!(
                    prefilter_of(pat).admits(sample),
                    "prefilter holds back a match of {} on {sample:?}",
                    pat.re.as_str()
                );
            }
        }
    }

    #[test]
    fn gates_agree_with_the_regexes_on_lookalike_letters() {
        // `regex_lite` folds ASCII case only: a Kelvin sign is not a `k`, a long
        // s is not an `s`, a full-width letter is not a Latin one. Whatever the
        // regexes decide about these, a gate must never disagree with them.
        for sample in [
            "to\u{212a}en=Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "\u{17f}ecret: Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "\u{ff21}uthorization: Bearer abcdefghijklmnopqrstuvwxyz",
            "authorization\u{a0}: Bearer abcdefghijklmnopqrstuvwxyz",
            "password\u{a0}= Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "PASSWORD\u{b}=\u{c}Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "éapi_key=Ab3xZ9Qw7Lm2Ns5Pt8Rv",
            "POSTGRES://app:s3cretP4ss@db.internal/kronn",
        ] {
            for pat in every_pattern() {
                assert!(
                    !pat.re.is_match(sample)
                        || (pat.gate.admits(sample) && prefilter_of(pat).admits(sample)),
                    "gate holds back a match of {} on {sample:?}",
                    pat.re.as_str()
                );
            }
            assert_eq!(
                redact_for_audit_artifact(sample),
                {
                    let (once, first) = apply_patterns_ungated(sample, PATTERNS.as_slice());
                    let (twice, second) = apply_patterns_ungated(&once, ASSIGNMENT.as_slice());
                    let changed = twice != sample;
                    (twice, if changed { (first + second).max(1) } else { 0 })
                },
                "{sample:?}"
            );
        }
    }

    #[test]
    fn gates_turn_away_prose_that_merely_mentions_secret_words() {
        // Mentions tokens, pins, passwords and keys without ever assigning one.
        let prose = "Review the mapping between the shipping token budget and the pin of each \
                     dependency. The password policy lives in the security page; never paste \
                     credentials. Open https://example.com/docs and compare the notes: key: value.";
        for pat in every_pattern() {
            assert!(
                !pat.gate.admits(prose),
                "{} would run on plain prose",
                pat.re.as_str()
            );
        }
        // …but the same words as an assignment go through to the regex.
        assert!(every_pattern().any(|pat| pat.gate.admits("the pin: 4321")));
    }

    #[test]
    fn prefilters_turn_away_short_identifiers_and_paths_in_one_scan() {
        // Names, flags, paths and ids: the bulk of what a resource holds.
        for text in [
            "--option-7-12",
            "value 7/12",
            "build/output",
            "step-3",
            "cargo",
            "",
            "x",
        ] {
            assert!(!VENDOR_PREFILTER.admits(text), "{text:?}");
            assert!(!ASSIGNMENT_PREFILTER.admits(text), "{text:?}");
            assert_eq!(redact_for_audit_artifact(text), (text.to_string(), 0));
        }
        // A text that starts any literal is let through (the gates then decide),
        // whatever the case, wherever it sits.
        for text in [
            "Bearer x",
            "the BEARER of news",
            "see Authorization: y",
            "sk-abc",
            "postgres://a:b@c",
            r#"{"token": "t"}"#,
        ] {
            assert!(VENDOR_PREFILTER.admits(text), "{text:?}");
        }
        assert!(ASSIGNMENT_PREFILTER.admits("a=b"));
        assert!(ASSIGNMENT_PREFILTER.admits("a: b"));
    }

    #[test]
    fn a_replacement_that_brings_a_literal_is_seen_by_the_patterns_after_it() {
        // `Authorization: Bearer …` is collapsed first, and its replacement holds
        // `Bearer` again for the pattern that follows: the prefilter is asked
        // afresh after each rewrite, not once on the input.
        let (redacted, count) = apply_patterns(
            "Authorization: Bearer abc123def456ghi789jkl and Bearer abcdefghijklmnopqrstuvwxyz",
            PATTERNS.as_slice(),
            &VENDOR_PREFILTER,
        );
        assert_eq!(
            redacted,
            "Authorization: Bearer ***REDACTED*** and Bearer ***REDACTED***"
        );
        assert_eq!(count, 2);
    }

    #[test]
    fn quoted_assignment_patterns_wait_for_their_own_quote() {
        // ASSIGNMENT is ordered: double quote, single quote, backtick, bare.
        let admitted = |text: &str| -> Vec<bool> {
            ASSIGNMENT.iter().map(|pat| pat.gate.admits(text)).collect()
        };
        // The quote sits before the name, not after the `:`: only the bare
        // value pattern has anything to do.
        assert_eq!(
            admitted("curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com"),
            [false, false, false, true]
        );
        assert_eq!(
            admitted("password: \"hunter2\""),
            [true, false, false, true]
        );
        assert_eq!(admitted("token =\n  'hunter2'"), [false, true, false, true]);
        assert_eq!(admitted("secret=`hunter2`"), [false, false, true, true]);
        // No assignment at all: nothing runs, whatever quotes the text holds.
        assert_eq!(admitted("say \"hello\" to `them`, it's fine"), [false; 4]);
        // Each pattern that is turned away really has no match to make.
        for text in [
            "curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com",
            "password: \"hunter2\"",
            "token =\n  'hunter2'",
            "secret=`hunter2`",
        ] {
            for pat in ASSIGNMENT.iter().filter(|pat| !pat.gate.admits(text)) {
                assert!(!pat.re.is_match(text), "{} on {text:?}", pat.re.as_str());
            }
        }
    }

    mod gate_equivalence {
        use super::*;
        use proptest::prelude::*;

        const FRAGMENTS: &[&str] = &[
            "APP_SECRET=",
            "password: ",
            "token=",
            "PIN =",
            "api-key:",
            "apiKey",
            "client_secret",
            "\"password\": \"",
            "\"token\":\"",
            "password: \"",
            "token='",
            "secret=`",
            "pin = \"\\\"",
            "Authorization: Bearer ",
            "authorization:basic ",
            "Bearer ",
            "Token ",
            "Basic ",
            "postgres://u:p@h",
            "redis://",
            "sk-",
            "p8e-",
            "AIza",
            "ghp_",
            "gho_",
            "xoxb-",
            "eyJ",
            "AKIA",
            "rk_live_",
            "pk_test_",
            "abcdefghijklmnopqrstuvwxyz",
            "0123456789ABCDEF",
            ".",
            "-",
            "_",
            "@",
            "://",
            " ",
            "\t",
            "\n",
            "\u{b}",
            "\u{a0}",
            "é",
            "\u{212a}",
            "\u{17f}",
            "secret",
            "TOKEN",
            "key",
            "value",
            "mapping",
            "pin",
            ":",
            "=",
            "'",
            "\"",
            "`",
            ",",
            ";",
            "\\",
            "***REDACTED***",
        ];

        fn text() -> impl Strategy<Value = String> {
            prop_oneof![
                proptest::collection::vec(proptest::sample::select(FRAGMENTS), 0..30)
                    .prop_map(|parts| parts.concat()),
                any::<String>(),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(2000))]

            #[test]
            fn a_gate_never_holds_back_a_match(input in text()) {
                for pat in every_pattern() {
                    prop_assert!(
                        !pat.re.is_match(&input) || pat.gate.admits(&input),
                        "gate of {} holds back a match on {:?}", pat.re.as_str(), input
                    );
                    prop_assert!(
                        !pat.re.is_match(&input) || prefilter_of(pat).admits(&input),
                        "prefilter of {} holds back a match on {:?}", pat.re.as_str(), input
                    );
                }
            }

            #[test]
            fn redaction_is_the_same_with_and_without_gates(input in text()) {
                prop_assert_eq!(
                    apply_patterns(&input, PATTERNS.as_slice(), &VENDOR_PREFILTER),
                    apply_patterns_ungated(&input, PATTERNS.as_slice())
                );
                prop_assert_eq!(
                    apply_patterns(&input, ASSIGNMENT.as_slice(), &ASSIGNMENT_PREFILTER),
                    apply_patterns_ungated(&input, ASSIGNMENT.as_slice())
                );
                // The whole audit-artifact pipeline, both passes chained.
                let (once, first) = apply_patterns_ungated(&input, PATTERNS.as_slice());
                let (twice, second) = apply_patterns_ungated(&once, ASSIGNMENT.as_slice());
                let changed = twice != input;
                prop_assert_eq!(
                    redact_for_audit_artifact(&input),
                    (twice, if changed { (first + second).max(1) } else { 0 })
                );
                prop_assert_eq!(
                    looks_like_secret(&input),
                    PATTERNS.iter().any(|pat| pat.re.is_match(&input))
                );
            }
        }
    }
}
