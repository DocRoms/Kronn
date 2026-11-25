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
    /// A match this says to leave is left as written, whatever the caller
    /// (KT-2039: the call that wraps a value another pattern already masked).
    leave: Option<LeaveMatch>,
}

/// See [`Pattern::leave`]: reads the match and the text it was found in.
type LeaveMatch = fn(&regex_lite::Captures<'_>, &str) -> bool;

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
    /// As [`Gate::Assignment`], for a value wrapped in a call (`Some("…")`,
    /// `Secret::new('…')`, KT-2039): the first byte past the `:`/`=` that is not
    /// up to a space opens an identifier, or a closure (`|| "…"`, KT-2053).
    WrappedAssignment,
    /// As [`Gate::Assignment`], for a quoted object key (`"NAME": "value"`,
    /// `'NAME': 'value'`): the name is followed by its closing quote (escaped or
    /// not) before the `:`. Carries the quote of the key, then the quote the
    /// value opens with (`"`, `'` or the `\` of `\"`).
    QuotedKey(u8, u8),
    /// A `://` followed, before any whitespace, by a `:` and then an `@`: the
    /// userinfo of a URL of any scheme (KT-2039).
    UserInfo,
}

impl Gate {
    fn admits(self, text: &str) -> bool {
        match self {
            Gate::Contains(needles) => needles.iter().any(|needle| text.contains(needle)),
            Gate::ContainsAsciiCi(needles) => {
                let lowered = text.to_ascii_lowercase();
                needles.iter().any(|needle| lowered.contains(needle))
            }
            Gate::Assignment(quote) => {
                assignment_gate(text, quote.map_or(Opener::Any, Opener::Quote), None)
            }
            Gate::WrappedAssignment => assignment_gate(text, Opener::Call, None),
            Gate::QuotedKey(key, quote) => assignment_gate(text, Opener::Quote(quote), Some(key)),
            Gate::UserInfo => userinfo_gate(text),
        }
    }

    /// The literals every text this gate admits contains, for
    /// [`Prefilter::literals`]; `None` when the gate is not of that shape.
    fn literals(self) -> Option<&'static [&'static str]> {
        match self {
            Gate::Contains(needles) | Gate::ContainsAsciiCi(needles) => Some(needles),
            Gate::UserInfo => Some(&["://"]),
            Gate::Assignment(_) | Gate::WrappedAssignment | Gate::QuotedKey(..) => None,
        }
    }
}

/// ASCII whitespace: the `\s` of `regex_lite`.
fn is_regex_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Gate of [`Gate::UserInfo`].
fn userinfo_gate(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find("://") {
        let start = from + at + 3;
        let mut colon = false;
        for byte in &bytes[start..] {
            match *byte {
                b':' => colon = true,
                b'@' if colon => return true,
                byte if is_regex_space(byte) => break,
                _ => {}
            }
        }
        from = start;
    }
    false
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
            let Some(needles) = gate.literals() else {
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
    //
    // The value never takes a `"` or a lone backslash: in JSON text the
    // header is followed by `\"` (inside a string) or `"` (ending it), and a
    // value that swallowed that backslash or quote would leave the document
    // unparseable (KT-1526). A backslash is part of the value only with the
    // character it escapes (`\\`, `\n`), so an escape is taken whole or not at
    // all. A value that opens with an escaped quote keeps it (group 3). An
    // escaped quote with a value character right behind it (`head\"tail`) is
    // inside the value, not its closing quote: JSON text never writes one.
    (
        r#"(?i)(authorization\s*:\s*)(bearer|basic|token|digest)\s+(\\")?(?:[^\s\\"]|\\[^\s"]|\\"[^\s\\"'`,;}\])])+"#,
        "$1$2 $3***REDACTED***",
        Gate::ContainsAsciiCi(&["authorization"]),
    ),
    // JSON-encoded credentials. `"password": "..."`, `"token": "..."`,
    // `"api_key"`, `"apiKey"`, `"secret"`, `"access_token"`, `"refresh_token"`,
    // `"client_secret"`, `"private_key"`. Ported from FE bug-report.ts
    // (one entry there) + expanded. The value reads an escaped quote (`\"`)
    // as part of it, up to the first bare quote. An excerpt cut inside the
    // value (maybe right after a backslash) has no closing quote: the value
    // then runs to the end of the text.
    (
        r#"("(?:password|token|api_key|apiKey|secret|access_token|refresh_token|client_secret|private_key)"\s*:\s*")(?:(?:\\[\s\S]|[^"\\])+\\?|\\)("|$)"#,
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
    // KT-2039 — the same for any other scheme (`rediss://`, `mssql://`,
    // `jdbc:sqlserver://`, `ftp://`), with or without a user (`redis://:pwd@`).
    // Neither part takes a `/`, which userinfo never holds unescaped: an `@`
    // further down a path (`https://host:443/@scope/pkg`) is no password.
    (
        r"(?i)\b([a-z][a-z0-9+.-]*)://([^:@\s/]*):([^@\s/]+)@",
        "$1://$2:***REDACTED***@",
        Gate::UserInfo,
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
    // GitHub fine-grained personal access tokens (`github_pat_<22>_<59>`): the
    // `gh*_` shape above does not read them (KT-1656).
    (
        r"\bgithub_pat_[A-Za-z0-9_]{30,}\b",
        "github_pat_***REDACTED***",
        Gate::Contains(&["github_pat_"]),
    ),
    // Google OAuth access (`ya29.…`) and refresh (`1//0…`) tokens, the ones of
    // `~/.gemini/oauth_creds.json` and of `gcloud` (KT-1656).
    (
        r"\bya29\.[A-Za-z0-9_-]{20,}",
        "ya29.***REDACTED***",
        Gate::Contains(&["ya29."]),
    ),
    (
        r"\b1//0[A-Za-z0-9_-]{40,}",
        "1//0***REDACTED***",
        Gate::Contains(&["1//0"]),
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
    // AWS temporary (STS) access keys: the `ASIA` twin of `AKIA` (KT-1656).
    (
        r"\bASIA[0-9A-Z]{16}\b",
        "ASIA***REDACTED***",
        Gate::Contains(&["ASIA"]),
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
    // NVIDIA API keys (`nvapi-` and 64 characters, KT-2039).
    (
        r"\bnvapi-[A-Za-z0-9_-]{20,}",
        "nvapi-***REDACTED***",
        Gate::Contains(&["nvapi-"]),
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
                    leave: None,
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
    apply_patterns_guarded(input, patterns, prefilter, None)
}

/// Decides whether a match is a credential worth masking, from its captures and
/// the text it was found in; a match it refuses is left as written.
type MatchGuard<'a> = &'a dyn Fn(&regex_lite::Captures<'_>, &str) -> bool;

/// [`apply_patterns`], with a `guard` that can leave a match as it is.
fn apply_patterns_guarded(
    input: &str,
    patterns: &[Pattern],
    prefilter: &Prefilter,
    guard: Option<MatchGuard<'_>>,
) -> (String, usize) {
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
            if guard.is_some_and(|guard| !guard(&captures, &out))
                || pat.leave.is_some_and(|leave| leave(&captures, &out))
            {
                continue;
            }
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
    redact_json_aware(input, |text| {
        apply_patterns(text, PATTERNS.as_slice(), &VENDOR_PREFILTER)
    })
    .0
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

/// The five catch-all-line patterns that open [`RAW_PATTERNS`]; every pattern
/// after them is a vendor token shape.
const CATCH_ALL_PATTERNS: usize = 5;

/// The vendor token shapes of [`RAW_PATTERNS`] as they are looked for in a KEY NAME
/// (KT-1528), DERIVED from it mechanically: the same body and the same minimum
/// lengths, only the `\b` on either side dropped (a label glued to a token,
/// `old_sk-…`, `myAKIA…`, `AKIA…_old`, hides it from an anchored pattern: `_` is a
/// word character), and the vendors' own case kept. A name is a few words, so a
/// token shape inside it is a secret pasted as a key. A new vendor pattern in
/// [`RAW_PATTERNS`] is therefore covered by name without a second list to update.
fn name_token_shapes() -> impl Iterator<Item = &'static str> {
    RAW_PATTERNS
        .iter()
        .skip(CATCH_ALL_PATTERNS)
        .map(|(src, _, _)| {
            let src = src.strip_prefix(r"\b").unwrap_or(src);
            src.strip_suffix(r"\b").unwrap_or(src)
        })
}

static NAME_TOKEN_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    name_token_shapes()
        .filter_map(|s| Regex::new(s).ok())
        .collect()
});

/// Whether a vendor token shape ([`name_token_shapes`]) appears in `name`,
/// anywhere, glued to a label or not. One exception: `sk-` inside an ordinary
/// word (`di|sk-cache-max-size-bytes`, `ta|sk-runner-…`) counts only when the
/// match holds a decimal digit or a capital, as every real `sk-` key does; a
/// kebab-case setting name holds neither. Every start is tried, so a rejected
/// match never hides a later one. The caller bounds the length of `name` (see
/// `config_salvage`).
pub fn name_embeds_vendor_token(name: &str) -> bool {
    NAME_TOKEN_REGEXES.iter().any(|re| {
        let word_prefix = re.as_str().starts_with("sk-");
        let mut from = 0;
        while let Some(m) = re.find_at(name, from) {
            let glued = name[..m.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphabetic());
            let token_like = m
                .as_str()
                .bytes()
                .any(|b| b.is_ascii_digit() || b.is_ascii_uppercase());
            if !(word_prefix && glued) || token_like {
                return true;
            }
            from = m.start() + name[m.start()..].chars().next().map_or(1, char::len_utf8);
        }
        false
    })
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
//   2. escaped-quoted: NAME [:=] \"…\" → the same inside JSON text, where the
//      quotes around a value are written `\"`. Keep the escaped quotes, mask
//      what they hold.
//   3. unquoted: NAME [:=] token → mask the token up to the next whitespace
//      / quote / separator. The token never ends in a backslash that escapes a
//      quote: in JSON text it would take the escape of the closing quote and
//      the document would no longer parse (KT-1526). A backslash belongs to the
//      token only with the character it escapes, so an escape is taken whole or
//      not at all (`\'` and `` \` `` are no JSON escapes and never end a token).
//      An escaped quote with a value character right behind it (`head\"tail`)
//      is inside the token too. A token may open with an escaped quote (a value
//      cut before its closing one), which is kept.
// Before them, KT-2053 review: the value EXPRESSION of every secret NAME
// ([`mask_value_expressions`]) has each of its quoted literals masked — `"…"`,
// `'…'`, backtick, `\"…\"`, prefixed (`b"…"`, `r#"…"#`, `f"…"`, `u'…'`), in
// any position: after a type (`token: &str = "…"`), a reference (`&"…"`), a
// method call (`os.getenv("X", "…")`), as a later argument
// (`env::var_or("KEY", "…")`). The expression runs from the `:` / `=` to a `,`
// `;` `)` `]` `}` at depth 0 or the end of the line ([`expression_literals`]).
// The shapes above then keep a call readable around a masked value
// (`Some("***REDACTED***")`) and mask a bare value.
// Anchored on the secret NAME, so ordinary path-keyed hex checksums (no NAME)
// are never touched. Quoted MUST run before unquoted (the unquoted value class
// excludes quotes, so it can't consume a quoted value itself).
const ASSIGNMENT_NAMES: &str = r"secret|apikey|api[_-]?key|passwd|password|pwd|pin|passcode|passphrase|access[_-]?key|private[_-]?key|signing[_-]?key|encryption[_-]?key|hmac[_-]?key|client[_-]?secret|token";

/// What can follow a secret word in a name that still holds the secret
/// (`SECRET_KEY`, `TOKEN_VALUE`, `passwordString`): KT-1656. A name is otherwise
/// read by its *last* word (`APP_SECRET`, `iris_token`).
const NAME_QUALIFIERS: &[&str] = &["key", "value", "string", "str"];

/// `name` (lowercase bytes) without a trailing qualifier of [`NAME_QUALIFIERS`]
/// and the whole run of `_` / `-` before it (`SECRET__KEY`, `secret--value`);
/// `None` when it has none, or nothing else.
fn without_qualifier(name: &[u8]) -> Option<&[u8]> {
    NAME_QUALIFIERS.iter().find_map(|qualifier| {
        let qualifier = qualifier.as_bytes();
        let split = name.len().checked_sub(qualifier.len())?;
        if !name[split..].eq_ignore_ascii_case(qualifier) {
            return None;
        }
        let stem = name[..split]
            .iter()
            .rposition(|byte| *byte != b'_' && *byte != b'-')
            .map_or(&name[..0], |last| &name[..=last]);
        (!stem.is_empty()).then_some(stem)
    })
}

/// Whether `name` ends with a secret word of [`ASSIGNMENT_NAMES`], with or
/// without a [`NAME_QUALIFIERS`] word after it.
fn ends_with_secret_word(name: &[u8], tails: &[Vec<u8>]) -> bool {
    let ends = |name: &[u8]| {
        tails.iter().any(|tail| {
            name.len() >= tail.len() && name[name.len() - tail.len()..].eq_ignore_ascii_case(tail)
        })
    };
    ends(name) || without_qualifier(name).is_some_and(ends)
}

/// The secret-ish name every assignment pattern opens with: a word ending in a
/// word of [`ASSIGNMENT_NAMES`], optionally followed by a [`NAME_QUALIFIERS`] one.
fn assignment_name() -> String {
    format!(
        "[a-z0-9_.-]*(?:{ASSIGNMENT_NAMES})(?:[_-]*(?:{NAME_QUALIFIERS}))?",
        NAME_QUALIFIERS = NAME_QUALIFIERS.join("|"),
    )
}

static ASSIGNMENT: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let name = assignment_name();
    let quoted_dq = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)\"((?:\\\\[\\s\\S]|[^\"\\\\])+)\"");
    let quoted_sq = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)'((?:\\\\[\\s\\S]|[^'\\\\])+)'");
    let quoted_bt = format!("(?i)(\\b{name}\\b\\s*[:=]\\s*)`((?:\\\\[\\s\\S]|[^`\\\\])+)`");
    // KT-2053 — the inside of a `\"…\"` value. It lives in a string printed with
    // `{:?}` (or in JSON text), so it is read as that string escaped once more:
    // a plain character, an escape other than `\\` (`\n`, `\t`), or `\\` with the
    // character it escapes in the value, itself written plain or escaped. A `"`
    // inside the value is then `\\\"`, read whole, and never the closing `\"`. A
    // bare `"` ends the printed string, so the value never takes one.
    let escaped_unit = r#"[^\\"]|\\[^\\"]|\\\\(?:\\[\s\S]|[^\\"])"#;
    // The same, cut before its closing quote: it may end on the `\\` of an
    // escape whose character was cut (`token: \"abc\\`).
    let escaped_cut = format!(r#"(?:{escaped_unit})+(?:\\\\)?|\\\\"#);
    let quoted_escaped = format!(r#"(?i)(\b{name}\b\s*[:=]\s*\\")((?:{escaped_unit})+)(\\")"#);
    // KT-2039 — a quoted value wrapped in a call, as `{:?}` prints an
    // `Option<String>` (`api_key: Some("…")`) and `{:#?}` across lines
    // (`password: Some(\n    "…",\n)`), or as code builds a secret type
    // (`token = Secret::new('…')`). The call is kept; the bare pattern below then
    // leaves it alone ([`call_of_a_masked_value`]). KT-2044: calls nest
    // (`Some(Token("…"))`, `Secret::new(Box::new('…'))`), so the opener repeats.
    // KT-2053: a space may come before the parenthesis (`strdup ("…")`), a path
    // segment may be a turbofish (`Secret::<String>::new(`; its types are a flat
    // class, so generics of any depth and lifetimes are read:
    // `::<Vec<Box<str>>>`, `::<&'static str>`), an opener may be a closure
    // (`unwrap_or_else(|| "…")`, `move |_|`), and the space after an opener may
    // be a printed `\n` / `\r` / `\t` (`{:#?}` inside a `{:?}`-printed string).
    // [`past_call_openers`], [`call_path_len`] and [`name_before_value`] read the
    // same syntax. These shapes only keep the call readable: every literal of the
    // value is masked by [`mask_value_expressions`] whatever its call.
    let ident = "[a-z_][a-z0-9_]*";
    let generic = r"<[a-z0-9_:, &'<>]*>";
    let path = format!("{ident}(?:::(?:{ident}|{generic}))*");
    let closure = r"(?:move[ \t]+)?\|[a-z0-9_, ]*\|";
    let call_space = r"(?:\s|\\[nrt])*";
    let call = format!(r"(?:(?:{path}[ \t]*\(|{closure}){call_space})+");
    let wrapped_dq = format!(r#"(?i)(\b{name}\b\s*[:=]\s*{call})"((?:\\[\s\S]|[^"\\])+)""#);
    let wrapped_sq = format!(r#"(?i)(\b{name}\b\s*[:=]\s*{call})'((?:\\[\s\S]|[^'\\])+)'"#);
    let wrapped_escaped =
        format!(r#"(?i)(\b{name}\b\s*[:=]\s*{call}\\")((?:{escaped_unit})+)(\\")"#);
    // A quoted value cut before its closing quote: the closed shapes above ran
    // first, so no closing quote is left to find — mask to the end of the text.
    // A lone backslash ending the text is part of the value (`\\?`); the quote
    // that opens it is kept, nothing is appended.
    let truncated = |quote: &str| {
        format!(r#"(?i)(\b{name}\b\s*[:=]\s*){quote}((?:\\[\s\S]|[^{quote}\\])+\\?|\\)$"#)
    };
    let truncated_dq = truncated("\"");
    let truncated_sq = truncated("'");
    let truncated_bt = truncated("`");
    // The escaped-quote form (`\"…`) lives in JSON text, so the string it opens
    // may end at its own bare closing quote: stop there, and keep that quote.
    let truncated_escaped = format!(r#"(?i)(\b{name}\b\s*[:=]\s*\\")({escaped_cut})(\\?$|")"#);
    // KT-2044 — the same inside a call (`password: Some("…` at the end of a
    // capped stream or block): the call and the opening quote are kept.
    let truncated_wrapped = |quote: &str| {
        format!(r#"(?i)(\b{name}\b\s*[:=]\s*{call}{quote})((?:\\[\s\S]|[^{quote}\\])+\\?|\\)$"#)
    };
    let truncated_wrapped_dq = truncated_wrapped("\"");
    let truncated_wrapped_sq = truncated_wrapped("'");
    let truncated_wrapped_escaped =
        format!(r#"(?i)(\b{name}\b\s*[:=]\s*{call}\\")({escaped_cut})(\\?$|")"#);
    let unquoted = format!(
        r#"(?i)(\b{name}\b\s*[:=]\s*(?:\\")?)((?:[^\s\\"'`,;]|\\[^\s"]|\\"[^\s\\"'`,;}}\])])+)"#
    );
    // KT-1656 — a credential file printed whole (`~/.gemini/settings.json`,
    // `.npmrc`-like JSON): the secret-ish name is a quoted object key, closed
    // by its quote before the `:`, which the shapes above never read.
    let json_key_dq = format!(r#"(?i)("{name}"\s*:\s*)"((?:\\[\s\S]|[^"\\])+)""#);
    let json_key_escaped = format!(r#"(?i)(\\"{name}\\"\s*:\s*\\")((?:{escaped_unit})+)(\\")"#);
    // KT-2039 — the same with single quotes, as Node's `util.inspect` prints an
    // object (`{ 'x-api-key': '…' }`) and Python a dict.
    let json_key_sq = format!(r#"(?i)('{name}'\s*:\s*)'((?:\\[\s\S]|[^'\\])+)'"#);
    // KT-2044 — a key in one quote and its value in the other: `util.inspect`
    // and Python's `repr` print `{ 'x-api-key': "it's…" }` when the value holds
    // a `'`, and a JS literal can read `{ "token": '…' }`.
    let json_key_sq_dq = format!(r#"(?i)('{name}'\s*:\s*)"((?:\\[\s\S]|[^"\\])+)""#);
    let json_key_dq_sq = format!(r#"(?i)("{name}"\s*:\s*)'((?:\\[\s\S]|[^'\\])+)'"#);
    // KT-2053 — the same inside a string printed with `{:?}`, where the double
    // quotes are escaped: `{ \"token\": '…' }`, `{ 'token': \"…\" }`. A
    // single-quoted value there is read as escaped once more, as `escaped_unit`
    // reads a `\"…\"` one (a `"` inside it is `\\\"`).
    let json_key_escaped_sq =
        format!(r#"(?i)(\\"{name}\\"\s*:\s*)'((?:[^\\']|\\[^\\]|\\\\(?:\\[\s\S]|[^\\]))+)'"#);
    let json_key_sq_escaped = format!(r#"(?i)('{name}'\s*:\s*\\")((?:{escaped_unit})+)(\\")"#);
    let leave_call: Option<LeaveMatch> = Some(call_of_a_masked_value);
    [
        (quoted_dq, "$1\"***REDACTED***\"", Gate::Assignment(Some(b'"')), None),
        (quoted_sq, "$1'***REDACTED***'", Gate::Assignment(Some(b'\'')), None),
        (quoted_bt, "$1`***REDACTED***`", Gate::Assignment(Some(b'`')), None),
        (quoted_escaped, "$1***REDACTED***$3", Gate::Assignment(Some(b'\\')), None),
        (wrapped_dq, "$1\"***REDACTED***\"", Gate::WrappedAssignment, None),
        (wrapped_sq, "$1'***REDACTED***'", Gate::WrappedAssignment, None),
        (wrapped_escaped, "$1***REDACTED***$3", Gate::WrappedAssignment, None),
        (truncated_dq, "$1\"***REDACTED***", Gate::Assignment(Some(b'"')), None),
        (truncated_sq, "$1'***REDACTED***", Gate::Assignment(Some(b'\'')), None),
        (truncated_bt, "$1`***REDACTED***", Gate::Assignment(Some(b'`')), None),
        (truncated_escaped, "$1***REDACTED***$3", Gate::Assignment(Some(b'\\')), None),
        (truncated_wrapped_dq, "$1***REDACTED***", Gate::WrappedAssignment, None),
        (truncated_wrapped_sq, "$1***REDACTED***", Gate::WrappedAssignment, None),
        (truncated_wrapped_escaped, "$1***REDACTED***$3", Gate::WrappedAssignment, None),
        (unquoted, "$1***REDACTED***", Gate::Assignment(None), leave_call),
        (json_key_dq, "$1\"***REDACTED***\"", Gate::QuotedKey(b'"', b'"'), None),
        (json_key_escaped, "$1***REDACTED***$3", Gate::QuotedKey(b'"', b'\\'), None),
        (json_key_sq, "$1'***REDACTED***'", Gate::QuotedKey(b'\'', b'\''), None),
        (json_key_sq_dq, "$1\"***REDACTED***\"", Gate::QuotedKey(b'\'', b'"'), None),
        (json_key_dq_sq, "$1'***REDACTED***'", Gate::QuotedKey(b'"', b'\''), None),
        (json_key_escaped_sq, "$1'***REDACTED***'", Gate::QuotedKey(b'"', b'\''), None),
        (json_key_sq_escaped, "$1***REDACTED***$3", Gate::QuotedKey(b'\'', b'\\'), None),
    ]
    .into_iter()
    .filter_map(|(src, repl, gate, leave)| {
        Regex::new(&src)
            .map(|re| Pattern {
                re,
                replacement: repl,
                gate,
                leave,
            })
            .map_err(|e| {
                tracing::warn!(pattern = %src, error = %e, "core::redact: invalid assignment pattern skipped");
            })
            .ok()
    })
    .collect()
});

/// KT-2039 / KT-2044 — whether a bare-value match (group 2) is only the calls
/// that wrap a quoted value the wrapped patterns already masked: it is then left
/// as written, so the mask reads `api_key: Some("***REDACTED***")` rather than
/// swallowing the `Some(`.
///
/// The text from group 2 on is read as call openers ([`past_call_openers`]:
/// `Some(`, `Some(Token(`, `Secret::new(`, `strdup (`, `Secret::<String>::new(`,
/// `unwrap_or_else(||`). That is the case when there is at least one and either
/// - group 2 goes on past them with only the escaped opening quote and the mask
///   (`Some(\"***REDACTED***`, inside a printed string), followed by the closing
///   quote (`\"`, or the bare `"` of the string around it) or by the end of the
///   text (a value cut before its closing quote);
/// - or group 2 ends within them (KT-2053: the bare value stops at the space of
///   `strdup (`, or at the `,` of a turbofish) and the text after them is a
///   closed mask (`"***REDACTED***"`) or a mask cut at the end of the text
///   (`"***REDACTED***`, left by the patterns of a wrapped value cut before its
///   closing quote). The openers take the whitespace after each of them,
///   `{:#?}` putting each on its own line.
///
/// Anything else after the call (a value not masked, nothing at all) has the
/// call masked with it.
fn call_of_a_masked_value(captures: &regex_lite::Captures<'_>, text: &str) -> bool {
    const MASKED: &[&str] = &[
        "\"***REDACTED***\"",
        "'***REDACTED***'",
        "`***REDACTED***`",
        "\\\"***REDACTED***\\\"",
    ];
    const MASKED_TO_THE_END: &[&str] = &["\"***REDACTED***", "'***REDACTED***"];
    let (Some(value), Some(matched)) = (captures.get(2), captures.get(0)) else {
        return false;
    };
    let from = &text[value.start()..];
    let (rest, openers) = past_call_openers(from);
    if openers == 0 {
        return false;
    }
    // The openers are ASCII: `consumed` is a character boundary.
    let consumed = from.len() - rest.len();
    // An escaped mask, then its closing quote (`\"`, or the bare `"` of the
    // string around it) or the end of the text (a value cut before it).
    let escaped_mask_ends = |text: &str| {
        text.strip_prefix(ESCAPED_MASK).is_some_and(|after| {
            after.starts_with('"') || after.starts_with("\\\"") || after.is_empty() || after == "\\"
        })
    };
    match value.as_str().get(consumed..) {
        Some(inside) if !inside.is_empty() => {
            inside == ESCAPED_MASK && escaped_mask_ends(&text[matched.end() - inside.len()..])
        }
        _ => {
            MASKED.iter().any(|masked| rest.starts_with(masked))
                || MASKED_TO_THE_END.contains(&rest)
                || escaped_mask_ends(rest)
        }
    }
}

/// The mask of a `\"…\"` value, from its opening quote.
const ESCAPED_MASK: &str = "\\\"***REDACTED***";

/// `text` past the call openers it starts with, and how many there were: the
/// `call` of [`ASSIGNMENT`], read by hand. An opener is a call path
/// ([`call_path_len`]) then `(`, with spaces or tabs between them (`strdup (`,
/// KT-2053), or a closure ([`closure_len`]); each takes the whitespace after
/// it, printed `\n` / `\r` / `\t` included ([`past_call_space`]).
fn past_call_openers(mut text: &str) -> (&str, usize) {
    let mut openers = 0;
    loop {
        let opener = closure_len(text).or_else(|| {
            let path = call_path_len(text);
            if path == 0 {
                return None;
            }
            let spaced = text[path..].trim_start_matches([' ', '\t']);
            spaced
                .starts_with('(')
                .then(|| text.len() - spaced.len() + 1)
        });
        let Some(length) = opener else {
            return (text, openers);
        };
        text = past_call_space(&text[length..]);
        openers += 1;
    }
}

/// A byte of a closure's parameters (`|x, _|`), as the `closure` of
/// [`ASSIGNMENT`] reads them.
fn closure_param_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b',' | b' ')
}

/// A byte inside a turbofish's angle brackets (`::<String>`,
/// `::<Vec<Box<str>>>`, `::<&'static str>`), as the flat class of the `generic`
/// of [`ASSIGNMENT`] reads it: nested angle brackets and lifetimes included.
fn generic_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'_' | b':' | b',' | b' ' | b'&' | b'\'' | b'<' | b'>')
}

/// Length of the closure opener `text` starts with (`||`, `|x|`, `move |_|`;
/// KT-2053), or `None`.
fn closure_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = 0;
    if bytes.len() > 4 && bytes[..4].eq_ignore_ascii_case(b"move") {
        let spaces = bytes[4..]
            .iter()
            .take_while(|byte| matches!(byte, b' ' | b'\t'))
            .count();
        if spaces > 0 && bytes.get(4 + spaces) == Some(&b'|') {
            at = 4 + spaces;
        }
    }
    if bytes.get(at) != Some(&b'|') {
        return None;
    }
    let params = bytes[at + 1..]
        .iter()
        .take_while(|byte| closure_param_byte(**byte))
        .count();
    (bytes.get(at + 1 + params) == Some(&b'|')).then_some(at + params + 2)
}

/// `text` past the whitespace after a call opener: ASCII whitespace, and the
/// `\n`, `\r`, `\t` escapes of a `{:#?}` dump printed again with `{:?}`.
fn past_call_space(mut text: &str) -> &str {
    loop {
        text = text.trim_start_matches(is_regex_space_char);
        match text.as_bytes() {
            [b'\\', escape, ..] if matches!(escape.to_ascii_lowercase(), b'n' | b'r' | b't') => {
                text = &text[2..];
            }
            _ => return text,
        }
    }
}

/// `text` without the whitespace a call opener may have after it, read from
/// the end ([`past_call_space`]).
fn before_call_space(mut text: &str) -> &str {
    loop {
        text = text.trim_end_matches(is_regex_space_char);
        match text.as_bytes() {
            [.., b'\\', escape] if matches!(escape.to_ascii_lowercase(), b'n' | b'r' | b't') => {
                text = &text[..text.len() - 2];
            }
            _ => return text,
        }
    }
}

/// Length of the call path `text` starts with (`Some`, `Secret::new`,
/// `Secret::<Vec<Box<str>>>::new`): ASCII identifiers joined by `::`, a segment
/// being a turbofish (KT-2053), as the `path` of [`ASSIGNMENT`] reads them; 0
/// when it starts with none. A turbofish is the longest run of [`generic_byte`]
/// after its `<`, up to the last `>` in it, as the greedy flat class of the
/// regex takes it.
fn call_path_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let identifier_end = |from: usize| -> Option<usize> {
        let first = bytes.get(from)?;
        if !(first.is_ascii_alphabetic() || *first == b'_') {
            return None;
        }
        let length = bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        Some(from + length)
    };
    let generic_end = |from: usize| -> Option<usize> {
        if bytes.get(from) != Some(&b'<') {
            return None;
        }
        let inside = &bytes[from + 1..];
        let run = inside
            .iter()
            .take_while(|byte| generic_byte(**byte))
            .count();
        inside[..run]
            .iter()
            .rposition(|byte| *byte == b'>')
            .map(|last| from + 2 + last)
    };
    let Some(mut end) = identifier_end(0) else {
        return 0;
    };
    while text[end..].starts_with("::") {
        match identifier_end(end + 2).or_else(|| generic_end(end + 2)) {
            Some(next) => end = next,
            None => break,
        }
    }
    end
}

/// `text` without the call path it ends with ([`call_path_len`] read from the
/// end): `Some`, `Secret::new`, `Secret::<String>::new`.
fn before_call_path(mut text: &str) -> &str {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    loop {
        text = text.trim_end_matches(word);
        if let Some(path) = text.strip_suffix("::") {
            text = path;
        } else if let Some(path) = before_turbofish(text) {
            text = path;
        } else {
            return text;
        }
    }
}

/// `text` without the turbofish it ends with (`::<String>`,
/// `::<Vec<Box<str>>>`, `::<&'static str>`): the run of [`generic_byte`] the
/// text ends with is cut at its first `::<`, where the flat class of the regex,
/// read from the left, opens it.
fn before_turbofish(text: &str) -> Option<&str> {
    if !text.ends_with('>') {
        return None;
    }
    let run = text
        .bytes()
        .rev()
        .take_while(|byte| generic_byte(*byte))
        .count();
    // The run is ASCII: its start is a character boundary.
    let start = text.len() - run;
    // `::` is part of the run (`:` is a type byte), so a `::<` is inside it.
    let opens = text[start..].find("::<")?;
    Some(&text[..start + opens])
}

/// `text` without the closure opener it ends with ([`closure_len`] read from
/// the end): `||`, `|x|`, `move |_|`.
fn before_closure(text: &str) -> Option<&str> {
    let before = text
        .strip_suffix('|')?
        .trim_end_matches(|c: char| u8::try_from(c).is_ok_and(closure_param_byte))
        .strip_suffix('|')?;
    let spaced = before.trim_end_matches([' ', '\t']);
    let bytes = spaced.as_bytes();
    if spaced.len() < before.len()
        && bytes.len() >= 4
        && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b"move")
    {
        let head = &spaced[..spaced.len() - 4];
        if !head.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
            return Some(head);
        }
    }
    Some(before)
}

fn is_regex_space_char(c: char) -> bool {
    u8::try_from(c).is_ok_and(is_regex_space)
}

// ── KT-2053 review — every literal of a secret's value expression ──────
//
// The patterns of [`ASSIGNMENT`] read a quoted value right after `NAME [:=]` or
// after the calls their `call` reads. Anything else in front of the literal
// (a type: `token: &str = "…"`; a prefix: `b"…"`, `r#"…"#`, `&"…"`; a method:
// `os.getenv("X", "…")`) or a second literal (`env::var_or("KEY", "…")`) used
// to leave it in clear. This pass reads the value expression itself: from the
// `:` / `=` to its end, every quoted literal inside it is masked.
//
// Round 2: a quote after `.` `!` `?` opens the value unless it closes a
// sentence (`$salt."…"`, `ok?"…":"x"` vs `… the key Y.", "b"`); a line whose
// next non-blank line starts with a continuation operator goes on there
// (`\n    .unwrap_or("…")`, `\n  || "…"`); a bare `"` after a `\"…\"` literal
// closes the `{:?}` string around the expression; and the pass stays linear
// on any input ([`CutMemo`], tails judged once).
//
// Round 3: the line also goes on when nothing was read since the `:` / `=`
// (`const token =\n  process.env.T ?? "…"`) or when it ends with an operator
// (`x ||\n  "…"`, `ok ?\n  "…" :`, `salt +`, `$salt .`, Go's `f(…).\n\tOr("…")`),
// as [`Carry`] reads from the last bytes of the line, tracked as it is read.
//
// Round 4: a trailing comment (`x || // default`, `x || # fallback`,
// `x || /* … */`) is blanks for how the line ends, so it no longer hides the
// operator before it; a `//` with no text after it is Perl's defined-or and
// carries the line ([`Comment`]).
//
// Round 5: a unified diff is also read in its old and new versions
// ([`diff_versions`]), so the carry that enters a `-` run enters the `+` run
// after it and a context line after both goes on from either; a line also
// goes on after the word `or` / `and`, a ` ..` or a `^` ([`LineEnd`]); `--`
// comments (Lua's `--[[ … ]]` too) are blanks like `//`, and after an operator
// any `#` opens a comment (`x || #[c]`); a bare `#"…"#` is a raw string; a
// literal that its comment line does not close is masked whole, but the next
// line is still read as code.
//
// Round 6: the mirror of round 5, a next line that starts with the word `or` /
// `and` (`local t = os.getenv("T")\n  or "…"`), Lua's `..` or Perl's `//`
// also goes on, after a trailing comment and in both diff versions too
// ([`continued_at`], [`leading_word_operator`]).
//
// Round 7: a line that holds only comments (`--`, `#`, `//`, `/* … */`, Lua's
// `--[[ … ]]`, a block across lines) is skipped like a blank one when looking
// for that leading operator (`x\n  -- c\n  or "…"`), in diff versions too, and
// its literals are still read ([`comment_run`]); Lua's long bracket `[[…]]` /
// `[==[…]==]` is a literal where a value starts (not `a[[1]]`).
//
// Second security review: a generic stays open past a line break while its
// list goes on (`Option<\n    String,\n> = …`, [`generic_goes_on`]), and the
// `>` of `->` / `=>` closes none; after an operator `.` / `?` / `!` (`$p.":"`,
// `??":…"`, a ternary `?" ":`) a quote opens a literal, whatever starts it;
// a comment opens glued to `:` `(` `[` `{` `,` `^` too (`:/* c */`, `(// c`);
// a line comment's literal stops at its line end; a `[[…]]` body's own
// literals are read too (a nested array); line starts are tracked forward
// ([`CutMemo::line_start`]), `\r` a line break as everywhere else.
//
// Round 9: a `,` at the value's own depth ends it only before a new key
// (`name =`, `name:`, `"name":`, `'name' =>`, [`key_follows`]); otherwise the
// value goes on (`token = x, "…"`, a Python tuple), and a line that ends with
// that `,` carries it like `||`. A line that starts with a `,` (comma-first
// style) continues the value and an open generic like a leading `||`.

/// The name a value expression follows, then its `:` / `=`: the name of
/// [`ASSIGNMENT`], optionally closed by its key quote, escaped or not
/// (`"token":`, `\"token\":`, `'token' =>`). Group 1 is the name.
static VALUE_EXPRESSION_NAME: LazyLock<Regex> = LazyLock::new(|| {
    let name = assignment_name();
    Regex::new(&format!(r#"(?i)\b({name})\b(?:\\?["'])?\s*[:=]"#))
        .expect("static value-expression regex")
});

/// Gate of [`VALUE_EXPRESSION_NAME`]: a secret-ish name, its key quote (escaped
/// or not) if any, optional whitespace, then `:` or `=` ([`assignment_gate`]
/// with any opener, for a bare name and both key quotes).
fn value_expression_gate(text: &str) -> bool {
    [None, Some(b'"'), Some(b'\'')]
        .into_iter()
        .any(|key_quote| assignment_gate(text, Opener::Any, key_quote))
}

/// Literal prefixes read before any quote (Rust `b`, `r`, `br`; Python `b`,
/// `r`, `f`, `u` and their pairs), lowercase.
const LITERAL_PREFIXES: &[&str] = &["b", "r", "br", "rb", "f", "fr", "rf", "u", "ur", "ru"];

/// Literal prefixes read before a double quote only (Rust `c`, `cr`; C++ `L`,
/// `u8`): before a `'` they are words (`l'été`).
const DOUBLE_QUOTE_PREFIXES: &[&str] = &["c", "cr", "l", "u8"];

/// Whether `word` is a literal prefix of `quote` ([`LITERAL_PREFIXES`],
/// [`DOUBLE_QUOTE_PREFIXES`]), ASCII case ignored.
fn literal_prefix(word: &str, quote: u8) -> bool {
    let known = |list: &[&str]| list.iter().any(|known| word.eq_ignore_ascii_case(known));
    known(LITERAL_PREFIXES) || (quote == b'"' && known(DOUBLE_QUOTE_PREFIXES))
}

/// An identifier byte for the expression reader: ASCII word bytes and every
/// byte of a non-ASCII character (`l'été`: an apostrophe, not a quote).
fn word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// The quoted literals inside the value expression that starts at `from` (right
/// after the `:` / `=` of a secret-ish name), as the ranges of their bodies
/// between their quotes, and where the expression ends. `memo` remembers the
/// literals already found to run to the end of the text ([`CutMemo`]).
///
/// The expression ends at a `;` `)` `]` `}` at depth 0 (brackets, parens,
/// braces opened inside it; a `<` right after a word or a `:` opens a generic,
/// closed by `>` but not the `>` of `->` / `=>`, or by anything that closes
/// what holds it, or by a line break unless its list goes on there:
/// [`generic_goes_on`]), at a `,` at depth 0 that a new key follows
/// (`, name = …`, `, "name": …`: [`key_follows`]; before anything else the
/// value goes on: `token = x, "…"`), or at the end of
/// the line: a line break, or a printed `\n` / `\r` (a `{:#?}` dump printed
/// with `{:?}`), at depth 0 only, as the call shapes read one inside a call.
/// The line does not end it when nothing but blanks was read since the `:` /
/// `=`, when the line ends with a continuation operator (`||`, `?`, `+`, `^`,
/// a `,`, the word `or` / `and`, a ` ..`…, [`LineEnd`], [`Carry::after`]), or
/// when the next non-blank line starts with one (`.unwrap_or(…)`, `|| "…"`,
/// `, "…"`), lines that hold only
/// comments between them skipped like blank ones ([`comment_run`]): [`Carry`],
/// [`continued_at`]. A comment (`//`, `#`, `--`, `/* … */` or Lua's
/// `--[[ … ]]` after a blank or an operator; after an operator any `#`, even
/// `#[` or `#!`; glued to `:` `(` `[` `{` `,` `^` too: [`comment_opens`])
/// counts as blanks for how the line ends, and
/// its brackets, `,` and `;` end nothing, but a literal inside it is still
/// read; one that its line does not close is masked to that line's end, and
/// reading goes on at that line's break, as code. A `//` with nothing after it (Perl's
/// defined-or) carries the line, and a block comment runs across line breaks
/// to its end. A diff's two versions are read by [`mask_value_expressions`].
/// It also ends at a `"`, `\"` or backtick that cannot open a literal (right
/// after a word, a closer or an escaped backslash; after a `.` `!` `?` when it
/// closes a sentence rather than opens a value, as [`read_literal`] tells: an
/// operator `.` `?` `!`, as in `$p.":"` or `??":…"`, never does): that
/// quote closes the string the expression is written in (`left: "Cfg { token:
/// abc }"`, `"… password=X and the key Y."`), as does a bare `"` once an
/// escaped literal was read (`left: "token: \"…\""`: the expression is inside
/// a string printed with `{:?}`, which that `"` closes).
///
/// A literal is `"…"`, `'…'`, a backtick one, or `\"…\"` (inside a string
/// printed with `{:?}`, read as escaped twice), after an optional prefix
/// (`b`, `r`, `f`, `u`…, [`LITERAL_PREFIXES`]) and, after an `r` or no word,
/// `#`s that its end repeats (`r#"…"#`, Swift's `#"…"#`), or Lua's long
/// bracket `[[…]]` / `[=[…]=]` (closed by the same number of `=`s, across
/// lines) where a value starts: first, or after an operator, an opener or the
/// word `or` / `and`, never after a name or a closer (`a[[1]]` is an index);
/// the literals inside a `[[…]]` are read too, as it may be a nested array
/// (`new Map([[k, "…"]])`: [`literals_within`]).
/// A `'` right after a word is an apostrophe (`it's`),
/// and after `&` or `<` (or a `,` in a generic) a lifetime (`&'static`). A
/// literal not closed runs to the end of the text, and an escaped one stops at
/// a bare `"` (the string around it ends there), which ends the expression too.
/// A quote right after a mask (`***REDACTED***"`) closes what that mask
/// replaced, never opens a literal, so a second pass changes nothing.
fn expression_literals(
    text: &str,
    from: usize,
    memo: &mut CutMemo,
) -> (Vec<std::ops::Range<usize>>, usize) {
    let bytes = text.as_bytes();
    let mut literals = Vec::new();
    // What each open bracket expects; a generic's `>` is "soft": anything that
    // closes what holds it closes it too.
    let mut open: Vec<u8> = Vec::new();
    let pop_generics = |open: &mut Vec<u8>| {
        while open.last() == Some(&b'>') {
            open.pop();
        }
    };
    // Whether an escaped literal was read: the expression is then inside a
    // string printed with `{:?}`, and a bare `"` closes that string.
    let mut printed = false;
    // How the line read so far ends ([`LineEnd`]), which [`Carry::after`]
    // reads at a line break without looking back, and whether the byte read
    // last was a blank.
    let mut line = LineEnd::default();
    let mut blank = false;
    // The comment being read, if any ([`Comment`]): blanks for `line`, and
    // its brackets, `,` and `;` end nothing.
    let mut comment: Option<Comment> = None;
    // Whether a `lambda` was read: an `ident:` after a `,` is then one of its
    // parameters, not a new key ([`key_follows`]).
    let mut lambda = false;
    let mut at = from;
    let mut steps = 0usize;
    let ends = loop {
        if at >= bytes.len() {
            break bytes.len();
        }
        steps += 1;
        let byte = bytes[at];
        let printed_break = byte == b'\\' && matches!(bytes.get(at + 1), Some(b'n' | b'r'));
        let line_break = matches!(byte, b'\n' | b'\r') || printed_break;
        if let Some(kind) = comment.filter(|kind| kind.spans_lines() || !line_break) {
            if let Some(length) = kind.closes_at(bytes, at) {
                comment = None;
                blank = true;
                at += length;
                continue;
            }
            let printed_blank = byte == b'\\' && bytes.get(at + 1) == Some(&b't');
            if !matches!(byte, b' ' | b'\t' | b'\n' | b'\r') && !printed_blank && !printed_break {
                if let Some(Comment::Line { text }) = comment.as_mut() {
                    *text = true;
                }
            }
            // A literal inside it is still read (fail-closed: `$x // "…"` is
            // Perl's defined-or, and what was masked before stays masked).
            let escaped = match byte {
                b'"' if printed => break at,
                b'"' | b'\'' | b'`' => false,
                b'\\' if bytes.get(at + 1) == Some(&b'"') => true,
                _ => {
                    at += if byte == b'\\' { 2 } else { 1 };
                    continue;
                }
            };
            let found = read_literal(text, from, at, &open, memo);
            // A literal of a line comment that its line does not close is
            // masked to the end of that line: the comment ends with its line,
            // and the next line is read as code (`x || #"c\n  "…"`), its line
            // break and code kept (KT-2053 second security review).
            if !kind.spans_lines() {
                if let Quote::Literal { body, .. } | Quote::Cut(body) = &found {
                    let line_end = memo.line_break_after(bytes, at + 1);
                    if line_end < body.end {
                        if line_end > body.start {
                            literals.push(body.start..line_end);
                        }
                        at = line_end;
                        continue;
                    }
                }
            }
            match found {
                Quote::Literal { body, next } => {
                    literals.push(body);
                    printed |= escaped;
                    at = next;
                }
                Quote::Cut(body) => {
                    let end = body.end;
                    literals.push(body);
                    break end;
                }
                Quote::Skip(next) => at = next,
                Quote::Ends => break at,
            }
            continue;
        }
        if let Some((kind, length)) = comment_opens(bytes, from, at, line.operator()) {
            comment = Some(kind);
            blank = true;
            at += length;
            continue;
        }
        // How the line ends, read at a line break: a line comment with no text
        // (`$ENV{X} //`) is Perl's defined-or, which carries anything.
        let carry = match comment {
            Some(Comment::Line { text: false }) => Carry::Any,
            _ => Carry::after(line),
        };
        if line_break {
            comment = None;
        }
        // Where reading goes on past a line break ([`continued_at`]): the code
        // there, or a comment-only line (KT-2053 round 7) read as a comment.
        let go_on = |next: Next, comment: &mut Option<Comment>, blank: &mut bool| match next {
            Next::Code(index) => index,
            Next::Comment { at, kind, length } => {
                *comment = Some(kind);
                *blank = true;
                at + length
            }
        };
        // How the line ended before this byte, for a Lua long bracket.
        let line_before = line;
        // Python's `lambda a, b: …`: its `b:` is a parameter, not a key.
        if byte == b'l'
            && (at == from || !word_byte(bytes[at - 1]))
            && bytes[at..].starts_with(b"lambda")
            && !bytes.get(at + 6).copied().is_some_and(word_byte)
        {
            lambda = true;
        }
        let printed_blank = byte == b'\\' && matches!(bytes.get(at + 1), Some(b'n' | b'r' | b't'));
        if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') || printed_blank {
            blank = true;
        } else {
            line.read(bytes, at, blank);
            blank = false;
        }
        let quote = match byte {
            b'(' => {
                open.push(b')');
                None
            }
            b'[' => {
                // KT-2053 round 7: Lua's long bracket `[[…]]` / `[=[…]=]` is a
                // literal where a value starts (first, or after an operator or
                // an opener), not after a name or a closer (`a[[1]]`).
                let equals = bytes[at + 1..]
                    .iter()
                    .take_while(|byte| **byte == b'=')
                    .count();
                expression_work(equals + 1);
                let previous = line_before.tail[1];
                let value_starts = previous == 0
                    || line_before.word_operator
                    || !(word_byte(previous)
                        || matches!(previous, b')' | b']' | b'}' | b'"' | b'\'' | b'`'));
                if value_starts && bytes.get(at + 1 + equals) == Some(&b'[') {
                    let close = format!("]{}]", "=".repeat(equals));
                    match scan_body(text, at + 2 + equals, b']', false, true, &close, memo) {
                        Quote::Literal { body, next } => {
                            literals.push(body.clone());
                            // A `[[` may also be a nested array (`new Map([[k,
                            // "…"]])`): the literals inside it are read too, so
                            // each is judged on its own (KT-2053 second
                            // security review).
                            if equals == 0 {
                                literals.extend(literals_within(text, body, &open, memo));
                            }
                            at = next;
                            continue;
                        }
                        Quote::Cut(body) => {
                            let end = body.end;
                            literals.push(body);
                            break end;
                        }
                        Quote::Skip(next) => {
                            at = next;
                            continue;
                        }
                        Quote::Ends => break at,
                    }
                }
                open.push(b']');
                None
            }
            b'{' => {
                open.push(b'}');
                None
            }
            b'<' => {
                if at > from && (word_byte(bytes[at - 1]) || bytes[at - 1] == b':') {
                    open.push(b'>');
                }
                None
            }
            b'>' => {
                // The `>` of `->` / `=>` closes no generic (`Result<fn() ->
                // String, E>`), unless that `-` is a diff line's marker.
                let arrow = at > from
                    && matches!(bytes[at - 1], b'-' | b'=')
                    && !(at - 1 == 0 || matches!(bytes[at - 2], b'\n' | b'\r'));
                if open.last() == Some(&b'>') && !arrow {
                    open.pop();
                }
                None
            }
            b')' | b']' | b'}' => {
                pop_generics(&mut open);
                match open.iter().rposition(|expected| *expected == byte) {
                    Some(index) => open.truncate(index),
                    None if open.is_empty() => break at,
                    None => {}
                }
                None
            }
            b';' => {
                pop_generics(&mut open);
                if open.is_empty() {
                    break at;
                }
                None
            }
            // KT-2053 round 9: a `,` at the value's own depth ends it only
            // before a new key (`token = "…", name = …`); otherwise the value
            // goes on (a Python tuple, a Ruby array: `token = x, "…"`).
            b',' => {
                if open.is_empty() && key_follows(bytes, at, lambda, memo) {
                    break at;
                }
                None
            }
            b'\n' | b'\r' => {
                if !generic_goes_on(bytes, &open, line, at, false, memo) {
                    pop_generics(&mut open);
                }
                if open.is_empty() {
                    match continued_at(text, at, false, carry, memo) {
                        Some(next) => {
                            at = go_on(next, &mut comment, &mut blank);
                            continue;
                        }
                        None => break at,
                    }
                }
                None
            }
            b'\\' => match bytes.get(at + 1) {
                Some(b'n' | b'r') => {
                    if !generic_goes_on(bytes, &open, line, at, true, memo) {
                        pop_generics(&mut open);
                    }
                    if open.is_empty() {
                        match continued_at(text, at, true, carry, memo) {
                            Some(next) => {
                                at = go_on(next, &mut comment, &mut blank);
                                continue;
                            }
                            None => break at,
                        }
                    }
                    at += 2;
                    continue;
                }
                Some(b'"') => Some(true),
                _ => {
                    at += 2;
                    continue;
                }
            },
            b'"' if printed => break at,
            b'"' | b'\'' | b'`' => Some(false),
            _ => None,
        };
        let Some(escaped) = quote else {
            at += 1;
            continue;
        };
        match read_literal(text, from, at, &open, memo) {
            Quote::Literal { body, next } => {
                literals.push(body);
                printed |= escaped;
                at = next;
            }
            Quote::Cut(body) => {
                let end = body.end;
                literals.push(body);
                break end;
            }
            Quote::Skip(next) => at = next,
            Quote::Ends => break at,
        }
    };
    expression_work(steps);
    (literals, ends)
}

/// The quoted literals inside `body`, the body of a Lua long bracket `[[…]]`
/// that may as well be a nested array (`[[k, "…"]]`), each cut at the end of
/// `body`; `open` are the brackets open around it. Reads `body` once (a scan
/// past its end stops at the closing quote met first, so it reads no byte
/// another scan of the same quote reads).
fn literals_within(
    text: &str,
    body: std::ops::Range<usize>,
    open: &[u8],
    memo: &mut CutMemo,
) -> Vec<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut at = body.start;
    while at < body.end {
        let quote = match bytes[at] {
            b'"' | b'\'' | b'`' => true,
            b'\\' => bytes.get(at + 1) == Some(&b'"'),
            _ => false,
        };
        if !quote {
            at += if bytes[at] == b'\\' { 2 } else { 1 };
            continue;
        }
        match read_literal(text, body.start, at, open, memo) {
            Quote::Literal { body: inner, next } if next <= body.end => {
                found.push(inner);
                at = next;
            }
            Quote::Literal { body: inner, .. } | Quote::Cut(inner) => {
                if inner.start < body.end {
                    found.push(inner.start..body.end);
                }
                break;
            }
            Quote::Skip(next) => at = next,
            Quote::Ends => at += 1,
        }
    }
    expression_work(at.min(body.end) - body.start + 1);
    found
}

/// Whether the generic open innermost (`open`) stays open past the line break
/// at `at` (a printed `\n` / `\r` when `printed`), KT-2053 second security
/// review: when its type list goes on, that is the line ends with the `<` that
/// opened it or a `,` of its list (`Option<\n    String,\n`), or the next line
/// starts with its `>` (`\n> = Some(…)`) or a `,` of its list (comma-first:
/// `String\n    , String`, KT-2053 round 9; in a diff, past the next line's
/// marker). Otherwise a line break closes it, as before: a `<` glued to a word
/// at the end of a line (`a<b`) opens nothing past it.
fn generic_goes_on(
    bytes: &[u8],
    open: &[u8],
    line: LineEnd,
    at: usize,
    printed: bool,
    memo: &mut CutMemo,
) -> bool {
    if open.last() != Some(&b'>') {
        return false;
    }
    if matches!(line.tail[1], b'<' | b',') {
        return true;
    }
    let diff = !printed && matches!(bytes.get(memo.line_start(bytes, at)), Some(b'+' | b'-'));
    // KT-2053 round 9: a next line that starts with a `,` goes on with the
    // list too (comma-first style: `HashMap<\n    String\n    , String\n>`).
    matches!(
        bytes.get(line_after(bytes, at, diff, printed)),
        Some(b'>' | b',')
    )
}

/// KT-2053 round 9: whether the segment after the `,` at `comma` (at the value
/// expression's own depth) starts a new key, so that the value ends at that
/// `,`: an identifier (after an optional `$`, `@` or Ruby's `:`, before an
/// optional `?` / `!`) or a quoted key (`"…"`, `'…'`, `\"…\"`), then blanks,
/// then a `:` (not `::`) or a `=` (not `==` / `=~`; `=>` is one) — `name =`,
/// `name=`, `name:`, `"name":`, `'name' =>`. After a `lambda` (`lambda_read`)
/// an `ident:` is one of its parameters, not a key. Blanks, line breaks (a
/// printed `\n` / `\r` / `\t`, a diff line's `+` / `-` marker) and line
/// comments between are skipped. Anything else (a value, a closer, a call, a
/// block comment, the end of the text) is no key, so the value goes on past
/// the `,` (fail-closed: `token = x, "…"`).
///
/// It reads the blanks and comments after the `,` and the key's first 64 bytes
/// at most; the next `,` the value reads comes after them, so each byte is
/// looked at by one `,` only and the pass stays linear.
fn key_follows(bytes: &[u8], comma: usize, lambda_read: bool, memo: &mut CutMemo) -> bool {
    let mut at = comma + 1;
    loop {
        match bytes.get(at) {
            Some(b' ' | b'\t') => at += 1,
            Some(b'\n' | b'\r') => {
                at += 1;
                // A diff line's marker (`--` / `++` is something else).
                if matches!(bytes.get(at), Some(b'+' | b'-')) && bytes.get(at + 1) != bytes.get(at)
                {
                    at += 1;
                }
            }
            Some(b'\\') if matches!(bytes.get(at + 1), Some(b'n' | b'r' | b't')) => at += 2,
            Some(_) => match comment_line_opens(bytes, at) {
                Some((kind @ Comment::Line { .. }, length)) => {
                    let end = comment_ends(bytes, at, kind, length, memo);
                    at = end.unwrap_or(bytes.len());
                }
                // A block comment may hide a line break, or not close: no key.
                Some(_) => {
                    expression_work(at - comma);
                    return false;
                }
                None => break,
            },
            None => break,
        }
    }
    expression_work(at - comma);
    key_at(bytes, at, lambda_read)
}

/// Whether a key and its `:` / `=` start at `at` ([`key_follows`]).
fn key_at(bytes: &[u8], at: usize, lambda_read: bool) -> bool {
    const LONGEST: usize = 64;
    let mut index = at;
    let quote: Option<&[u8]> = match bytes.get(index) {
        Some(b'"') => Some(b"\""),
        Some(b'\'') => Some(b"'"),
        Some(b'\\') if bytes.get(index + 1) == Some(&b'"') => Some(b"\\\""),
        _ => None,
    };
    match quote {
        Some(quote) => {
            index += quote.len();
            let start = index;
            while index - start < LONGEST
                && bytes
                    .get(index)
                    .is_some_and(|byte| !matches!(byte, b'"' | b'\'' | b'\\' | b'\n' | b'\r'))
            {
                index += 1;
            }
            if index == start || !bytes[index..].starts_with(quote) {
                expression_work(index - at + 1);
                return false;
            }
            index += quote.len();
        }
        None => {
            if matches!(bytes.get(index), Some(b'$' | b'@' | b':')) {
                index += 1;
            }
            let start = index;
            while index - start <= LONGEST && bytes.get(index).copied().is_some_and(word_byte) {
                index += 1;
            }
            if index == start || index - start > LONGEST {
                expression_work(index - at + 1);
                return false;
            }
            if matches!(bytes.get(index), Some(b'?' | b'!')) && bytes.get(index + 1) == Some(&b':')
            {
                index += 1;
            }
        }
    }
    while matches!(bytes.get(index), Some(b' ' | b'\t')) && index - at <= 2 * LONGEST {
        index += 1;
    }
    expression_work(index - at + 2);
    match (bytes.get(index), bytes.get(index + 1)) {
        (Some(b':'), next) => next != Some(&b':') && !(lambda_read && quote.is_none()),
        (Some(b'='), next) => !matches!(next, Some(b'=' | b'~')),
        _ => false,
    }
}

/// A comment read by [`expression_literals`] (KT-2053 round 4): its bytes are
/// blanks for how the line ends, so `x || // default` still ends with `||`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Comment {
    /// `// …`, `# …` or `-- …`, to the end of the line; `text` once anything
    /// but blanks was read in it (a `//` with none is Perl's defined-or
    /// operator; an empty `#` or `--` carries the line too, fail-closed).
    Line { text: bool },
    /// `/* … */`, across line breaks, to its `*/` (or the end of the text).
    Block,
    /// Lua's `--[[ … ]]` (`--[==[ … ]==]` with `equals` `=`s), across line
    /// breaks, to the long bracket that closes it (or the end of the text).
    LongBracket { equals: usize },
}

impl Comment {
    fn spans_lines(self) -> bool {
        !matches!(self, Self::Line { .. })
    }

    /// The length of what closes this comment at `at`, if it closes there:
    /// `*/` for a block, `]` `=`… `]` for a long bracket.
    fn closes_at(self, bytes: &[u8], at: usize) -> Option<usize> {
        match self {
            Self::Line { .. } => None,
            Self::Block => (bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/')).then_some(2),
            Self::LongBracket { equals } => {
                if bytes[at] != b']' {
                    return None;
                }
                // Each `=` follows at most one `]` right before it: linear.
                let run = bytes[at + 1..]
                    .iter()
                    .take(equals)
                    .take_while(|byte| **byte == b'=')
                    .count();
                expression_work(run);
                (run == equals && bytes.get(at + 1 + equals) == Some(&b']')).then_some(equals + 2)
            }
        }
    }
}

/// The comment that starts at `at` in the expression that started at `from`,
/// and the length of its opening marker: a `//`, `/*`, `--` (`--[[` / `--[=[`
/// a Lua long comment) or `#` right after the `:` / `=`, a blank, a line break
/// or an operator (`|` `&` `?` `+` `=`), so that a URL (`https://x`), a word
/// (`C#`) or a decrement (`i--`) is none. A `#"`, `##`, `#[` or `#!` (a raw
/// string, an attribute, a shebang) is no comment either, unless the line so
/// far ends with an operator that needs a right-hand side (`after_operator`:
/// after `x || `, nothing but a comment starts with `#`). Glued to a `:` `(`
/// `[` `{` `,` `^`, a `/*` opens one, and a `//` `--` `#` when a blank or the
/// line's end follows it (a `//` after anything but `:` always: `://x` is a
/// URL).
fn comment_opens(
    bytes: &[u8],
    from: usize,
    at: usize,
    after_operator: bool,
) -> Option<(Comment, usize)> {
    let previous = (at > from).then(|| bytes[at - 1]);
    let after = previous.is_none_or(|byte| {
        matches!(
            byte,
            b' ' | b'\t' | b'\n' | b'\r' | b'|' | b'&' | b'?' | b'+' | b'='
        )
    });
    if !after {
        // KT-2053 second security review: glued to a `:` `(` `[` `{` `,` or
        // `^` (`a :/* c */`, `(// c`), a comment opens too. There a `/*`
        // always does; a `//`, `--` or `#` when a blank or the line's end
        // follows it (not a URL's `://x`, a decrement `(--i`, a CSS
        // `color:#fff`, an attribute `(#[x]` or a raw string `(#"…"#`); a
        // `//` after anything but a `:` whatever follows it.
        let glued =
            previous.is_some_and(|byte| matches!(byte, b':' | b'(' | b'[' | b'{' | b',' | b'^'));
        if !glued {
            return None;
        }
        let spaced = |index: usize| match bytes.get(index) {
            None | Some(b' ' | b'\t' | b'\n' | b'\r') => true,
            Some(b'\\') => matches!(bytes.get(index + 1), Some(b'n' | b'r' | b't')),
            _ => false,
        };
        return match (bytes[at], bytes.get(at + 1)) {
            (b'/', Some(b'*')) => Some((Comment::Block, 2)),
            (b'/', Some(b'/')) if previous != Some(b':') || spaced(at + 2) => {
                Some((Comment::Line { text: false }, 2))
            }
            (b'-', Some(b'-')) => match long_comment(bytes, at) {
                Some(found) => Some(found),
                None => spaced(at + 2).then_some((Comment::Line { text: false }, 2)),
            },
            (b'#', _) if spaced(at + 1) => Some((Comment::Line { text: false }, 1)),
            _ => None,
        };
    }
    match (bytes[at], bytes.get(at + 1)) {
        (b'/', Some(b'/')) => Some((Comment::Line { text: false }, 2)),
        (b'/', Some(b'*')) => Some((Comment::Block, 2)),
        (b'-', Some(b'-')) => long_comment(bytes, at).or(Some((Comment::Line { text: false }, 2))),
        (b'#', Some(b'"' | b'#' | b'[' | b'!')) if !after_operator => None,
        (b'#', _) => Some((Comment::Line { text: false }, 1)),
        _ => None,
    }
}

/// Lua's long comment `--[[` / `--[=[` (`equals` `=`s) at `at`, and the length
/// of its marker.
fn long_comment(bytes: &[u8], at: usize) -> Option<(Comment, usize)> {
    if bytes.get(at + 2) != Some(&b'[') {
        return None;
    }
    let equals = bytes[at + 3..]
        .iter()
        .take_while(|byte| **byte == b'=')
        .count();
    expression_work(equals);
    (bytes.get(at + 3 + equals) == Some(&b'['))
        .then_some((Comment::LongBracket { equals }, equals + 4))
}

/// How the line a value expression has read so far ends, outside comments
/// ([`Comment`]): its last two non-blank bytes (`[before, last]`, `0` for none
/// read), whether blanks came right before each (`$salt .`, `"sk_" ..`), and
/// whether its last word is the operator `or` / `and` (ASCII case ignored).
/// Kept up to date byte by byte ([`LineEnd::read`]), never read backwards.
#[derive(Clone, Copy, Default, Debug)]
struct LineEnd {
    tail: [u8; 2],
    spaced: [bool; 2],
    /// The last run of word bytes, while it is the last non-blank thing read.
    word: Option<(usize, usize)>,
    word_operator: bool,
}

impl LineEnd {
    /// Reads the non-blank byte at `at`, after a blank when `spaced`.
    fn read(&mut self, bytes: &[u8], at: usize, spaced: bool) {
        let byte = bytes[at];
        self.tail = [self.tail[1], byte];
        self.spaced = [self.spaced[1], spaced];
        self.word = word_byte(byte).then(|| match self.word {
            Some((start, end)) if end == at => (start, at + 1),
            _ => (at, at + 1),
        });
        self.word_operator = self.word.is_some_and(|(start, end)| {
            let word = &bytes[start..end];
            word.eq_ignore_ascii_case(b"or") || word.eq_ignore_ascii_case(b"and")
        });
    }

    /// Whether the line ends with an operator that needs a right-hand side:
    /// `||` `&&` `?` (so `??`) `:` (so `::`) `+` `=` (so `==`) `^` (cmd's
    /// line continuation), Perl's `//` (`$ENV{X}//`), a `.` after a blank
    /// (`$salt .`: no sentence ends that way), a `..` after a blank (Lua's
    /// `"sk_" ..`), or the word `or` / `and`.
    fn operator(self) -> bool {
        self.word_operator
            || match self.tail {
                [_, b'?' | b':' | b'+' | b'=' | b'^'] => true,
                [before, last @ (b'|' | b'&' | b'/')] => before == last,
                [_, b'.'] if self.spaced[1] => true,
                [b'.', b'.'] => self.spaced[0],
                _ => false,
            }
    }
}

/// What the line a value expression has read so far lets the next line be,
/// from how that line ends ([`Carry::after`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Carry {
    /// Anything: nothing but blanks was read since the `:` / `=` (the value is
    /// on the next line: `const token =\n  process.env.T ?? "…"`), or the line
    /// ends with an operator that needs a right-hand side (`x ||\n  "…"`).
    Any,
    /// A chained call only: the line ends with a `.` (Go's `f(…).\n\tOr("…")`),
    /// which in prose ends a sentence (`abc.\nAnd then …`).
    Call,
    /// A line that starts with a continuation operator only ([`continued_at`]).
    Leading,
}

impl Carry {
    /// How a line that ends as `line` does carries on: nothing read, a
    /// trailing operator ([`LineEnd::operator`]: `||`, `??`, `+`, `or`, ` ..`,
    /// `^`…), a `(` or a `,` (one at the value's depth is read only when no
    /// new key follows it: [`key_follows`]) carry anything; a trailing `.` (not `..`) right after a
    /// word or a closer carries a chained call; anything else a leading
    /// operator. A trailing comment is not part of `line` ([`Comment`]).
    fn after(line: LineEnd) -> Self {
        match line.tail {
            // A `,` at the value's depth that no new key follows (KT-2053
            // round 9: `token = x,\n  "…"`).
            [_, 0 | b'(' | b','] => Self::Any,
            _ if line.operator() => Self::Any,
            [before, b'.'] if before != b'.' => Self::Call,
            _ => Self::Leading,
        }
    }
}

/// Where a value expression goes on past the line break at `at` (a real one, or
/// a printed `\n` / `\r` when `printed`), given how its line ends (`carry`):
/// the start of the next non-blank line when that line may continue it, `None`
/// when it may not, so the expression ends at the break.
///
/// With [`Carry::Any`] any next line continues it; with [`Carry::Call`] a line
/// that starts with an identifier right before `(` (a chained call: `Or("…")`,
/// not `And then …`); with [`Carry::Leading`] a line that starts with an
/// operator a formatter puts at the start of a continued line (rustfmt's
/// `.unwrap_or(…)` chains, a leading `|| "…"`, Lua's and Perl's leading
/// `or "…"`): `.` and Lua's `..` (not `...`), `?.`, `??`, the `?` and `:` of a
/// ternary (not `::`), `||`, `&&`, `+`, a `,` (comma-first style, KT-2053
/// round 9; a new key after it still ends the value: [`key_follows`]),
/// Perl's `//` (a comment line read as
/// one masks its literals, fail-closed), and the word `or` / `and`
/// ([`leading_word_operator`]). Otherwise a word, a closer or a `{:#?}` field
/// ends the expression there, as does plain prose. How the line ends is judged
/// on its code, a trailing comment left out ([`Comment`]).
/// In a unified diff (the line of the break starts with `+` or `-`) the next
/// line's own `+` / `-` / ` ` marker is skipped first, and only that line is
/// looked at. Where that line starts is tracked forward
/// ([`CutMemo::line_start`], past a `\n` or a `\r`), never searched backwards.
///
/// With [`Carry::Leading`], a line that holds only comments (`--`, `#`, `//`,
/// `/* … */` or Lua's `--[[ … ]]`, either running across lines) is skipped like
/// a blank one when looking for that operator (KT-2053 round 7: `x\n  -- c\n
/// or "…"`), [`comment_run`]; the expression then goes on with that comment
/// ([`Next::Comment`]), so its literals are read too.
fn continued_at(
    text: &str,
    at: usize,
    printed: bool,
    carry: Carry,
    memo: &mut CutMemo,
) -> Option<Next> {
    let bytes = text.as_bytes();
    // Tracked forward ([`CutMemo::line_start`]), `\r` a line break too.
    let line_start = (!printed).then(|| memo.line_start(bytes, at));
    let diff = line_start.is_some_and(|start| matches!(bytes.get(start), Some(b'+' | b'-')));
    let index = line_after(bytes, at, diff, printed);
    let first = *bytes.get(index)?;
    let continues = match carry {
        Carry::Any => true,
        Carry::Call => {
            let name = bytes[index..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
                .count();
            expression_work(name + 1);
            name > 0 && !first.is_ascii_digit() && bytes.get(index + name) == Some(&b'(')
        }
        Carry::Leading => {
            if !leading_operator(bytes, index) {
                return comment_run(bytes, at, index, diff, printed, memo);
            }
            true
        }
    };
    continues.then_some(Next::Code(index))
}

/// What a value expression goes on with past a line break ([`continued_at`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Next {
    /// The code at this index.
    Code(usize),
    /// A comment-only line before a line that starts with a continuation
    /// operator: the comment of kind `kind` whose `length`-byte marker starts
    /// at `at`.
    Comment {
        at: usize,
        kind: Comment,
        length: usize,
    },
}

/// The first byte after the line break at `at` and the blanks after it: in a
/// `diff`, one line break, the next line's `+` / `-` / ` ` marker, then its
/// indentation; otherwise every blank and line break (a printed `\n` / `\r` /
/// `\t` too when `printed`).
fn line_after(bytes: &[u8], at: usize, diff: bool, printed: bool) -> usize {
    let mut index = at;
    let blank = |index: usize, newlines: bool| match bytes.get(index) {
        Some(b' ' | b'\t') => Some(1),
        Some(b'\r' | b'\n') if newlines => Some(1),
        Some(b'\\') if printed && matches!(bytes.get(index + 1), Some(b'n' | b'r' | b't')) => {
            Some(2)
        }
        _ => None,
    };
    if diff {
        if bytes.get(index) == Some(&b'\r') {
            index += 1;
        }
        if bytes.get(index) == Some(&b'\n') {
            index += 1;
        }
        if matches!(bytes.get(index), Some(b'+' | b'-' | b' ')) {
            index += 1;
        }
        while let Some(width) = blank(index, false) {
            index += width;
        }
    } else {
        while let Some(width) = blank(index, true) {
            index += width;
        }
    }
    expression_work(index - at);
    index
}

/// Whether the line whose first non-blank byte is at `index` starts with an
/// operator a formatter puts at the start of a continued line
/// ([`continued_at`], [`Carry::Leading`]).
fn leading_operator(bytes: &[u8], index: usize) -> bool {
    let Some(&first) = bytes.get(index) else {
        return false;
    };
    let next = bytes.get(index + 1).copied();
    match first {
        // `.` and Lua's `..`, not a `...` (a spread, or an ellipsis).
        b'.' => next != Some(b'.') || bytes.get(index + 2) != Some(&b'.'),
        // A leading `,` (comma-first style, KT-2053 round 9): a new key after
        // it still ends the value there ([`key_follows`]).
        b'?' | b'+' | b',' => true,
        b':' => next != Some(b':'),
        b'|' => next == Some(b'|'),
        b'&' => next == Some(b'&'),
        // Perl's defined-or (a comment line read as one fails closed).
        b'/' => next == Some(b'/'),
        // `or` / `and` as a whole word (Lua, Perl, Ruby, SQL).
        b'o' | b'O' | b'a' | b'A' => leading_word_operator(bytes, index),
        _ => false,
    }
}

/// The comment that starts a comment-only line at `at` and the length of its
/// marker: `/*`, `//`, `--` (`--[[` / `--[=[` a Lua long comment) or any `#`.
fn comment_line_opens(bytes: &[u8], at: usize) -> Option<(Comment, usize)> {
    match (bytes.get(at)?, bytes.get(at + 1)) {
        (b'/', Some(b'*')) => Some((Comment::Block, 2)),
        (b'/', Some(b'/')) => Some((Comment::Line { text: true }, 2)),
        (b'-', Some(b'-')) => long_comment(bytes, at).or(Some((Comment::Line { text: true }, 2))),
        (b'#', _) => Some((Comment::Line { text: true }, 1)),
        _ => None,
    }
}

/// Where the comment of kind `kind` whose `length`-byte marker starts at `at`
/// ends: the line break that ends a line comment (or the end of the text), or
/// right past the `*/` / long bracket that closes a block; `None` for a block
/// not closed.
fn comment_ends(
    bytes: &[u8],
    at: usize,
    kind: Comment,
    length: usize,
    memo: &mut CutMemo,
) -> Option<usize> {
    match kind {
        Comment::Line { .. } => Some(memo.line_break_after(bytes, at + length)),
        Comment::Block => memo
            .find_after(bytes, b"*/", at + length)
            .map(|end| end + 2),
        Comment::LongBracket { equals } => {
            let mut close = vec![b']'];
            close.extend(std::iter::repeat_n(b'=', equals));
            close.push(b']');
            memo.find_after(bytes, &close, at + length)
                .map(|end| end + close.len())
        }
    }
}

/// KT-2053 round 7: from the line break at `at`, whose next line starts at
/// `index` with neither a continuation operator nor code, the lines that hold
/// only comments ([`comment_line_opens`]; several on a line, a block across
/// lines) are crossed like blank ones. When the first line after them starts
/// with a continuation operator ([`leading_operator`]), the expression goes on
/// with the first of those comments ([`Next::Comment`]), else it ends at `at`.
///
/// What is found after each line break crossed is remembered ([`CutMemo`]):
/// [`expression_literals`] reads those comments and asks again at each of their
/// line breaks, and an expression of a name inside a comment of the run asks
/// from a break of the run, so each is answered at once and the pass stays
/// linear.
fn comment_run(
    bytes: &[u8],
    at: usize,
    index: usize,
    diff: bool,
    printed: bool,
    memo: &mut CutMemo,
) -> Option<Next> {
    if let Some(known) = memo.2.get(&at) {
        return *known;
    }
    comment_line_opens(bytes, index)?;
    // Each line break crossed, with what starts the line after it.
    let mut crossed: Vec<(usize, Next)> = Vec::new();
    let (mut line_break, mut start) = (at, index);
    let reaches = 'lines: loop {
        if line_break != at {
            if let Some(known) = memo.2.get(&line_break) {
                break known.is_some();
            }
        }
        if leading_operator(bytes, start) {
            crossed.push((line_break, Next::Code(start)));
            break true;
        }
        let Some((kind, length)) = comment_line_opens(bytes, start) else {
            break false;
        };
        crossed.push((
            line_break,
            Next::Comment {
                at: start,
                kind,
                length,
            },
        ));
        // The rest of the line: comments and blanks, then its break, or a
        // continuation operator (`/* c */ || "…"`).
        let (mut comment_at, mut comment) = (start, (kind, length));
        line_break = loop {
            let Some(mut end) = comment_ends(bytes, comment_at, comment.0, comment.1, memo) else {
                break 'lines false;
            };
            let from = end;
            while matches!(bytes.get(end), Some(b' ' | b'\t'))
                || (bytes.get(end) == Some(&b'\\') && bytes.get(end + 1) == Some(&b't'))
            {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            expression_work(end - from + 1);
            match bytes.get(end) {
                None => break 'lines false,
                Some(b'\n' | b'\r') => break end,
                Some(b'\\') if matches!(bytes.get(end + 1), Some(b'n' | b'r')) => break end,
                Some(_) if leading_operator(bytes, end) => break 'lines true,
                Some(_) => match comment_line_opens(bytes, end) {
                    Some(next) => (comment_at, comment) = (end, next),
                    None => break 'lines false,
                },
            }
        };
        let printed_break = printed || bytes[line_break] == b'\\';
        start = line_after(bytes, line_break, diff, printed_break);
    };
    let answer = crossed
        .first()
        .and_then(|(_, next)| reaches.then_some(*next));
    for (line_break, next) in crossed {
        memo.2.insert(line_break, reaches.then_some(next));
    }
    answer
}

/// Whether the line that starts at `index` starts with the word operator `or`
/// / `and` (KT-2053 round 6: `local t = os.getenv("T")\n  or "…"`), a whole
/// word ([`word_byte`]: not `order`, `android`, `or_else`). In lower or upper
/// case it always is one (`or`, `OR`); a capitalised `Or` / `And` starts a
/// sentence in prose (`And then she said "hello"`), so it is one only when no
/// letter follows it, within a few blanks (`Or "…"`, `And(…)`, `Or $x`, or the
/// end of the line; past the look, fail-closed). Reads at most 16 bytes.
fn leading_word_operator(bytes: &[u8], index: usize) -> bool {
    let length = bytes[index..]
        .iter()
        .take(4)
        .take_while(|byte| word_byte(**byte))
        .count();
    let word = &bytes[index..index + length];
    if !(word.eq_ignore_ascii_case(b"or") || word.eq_ignore_ascii_case(b"and")) {
        expression_work(length + 1);
        return false;
    }
    if word.iter().all(u8::is_ascii_lowercase) || word.iter().all(u8::is_ascii_uppercase) {
        expression_work(length + 1);
        return true;
    }
    let after = bytes[index + length..]
        .iter()
        .take(12)
        .find(|byte| !matches!(byte, b' ' | b'\t'));
    expression_work(16);
    !after.is_some_and(u8::is_ascii_alphabetic)
}

/// What a quote met by [`expression_literals`] is.
enum Quote {
    /// A literal: its body, and where reading resumes past its closing quote.
    Literal {
        body: std::ops::Range<usize>,
        next: usize,
    },
    /// A literal not closed, or an escaped one cut by a bare `"`: its body runs
    /// to where the expression ends.
    Cut(std::ops::Range<usize>),
    /// An apostrophe or a lifetime: reading resumes there.
    Skip(usize),
    /// The quote of the string around the expression: it ends there.
    Ends,
}

/// Reads the quote at `at` (`"`, `'`, a backtick, or the `\` of `\"`) of the
/// expression that started at `from`; `open` are its open brackets.
fn read_literal(text: &str, from: usize, at: usize, open: &[u8], memo: &mut CutMemo) -> Quote {
    let bytes = text.as_bytes();
    let escaped = bytes[at] == b'\\';
    let quote = if escaped { b'"' } else { bytes[at] };
    // The word right before `end` (a run of [`word_byte`], so it starts on a
    // character boundary), and whether it is a literal prefix of this quote.
    let word_before = |end: usize| {
        let mut length = bytes[from..end]
            .iter()
            .rev()
            .take_while(|byte| word_byte(**byte))
            .count();
        // A printed `\n` / `\r` / `\t` before it is whitespace, not a word
        // (`Some(\n\"…\"`): an odd run of backslashes escapes its letter.
        let backslashes = bytes[from..end - length]
            .iter()
            .rev()
            .take_while(|byte| **byte == b'\\')
            .count();
        expression_work(length + backslashes + 1);
        if backslashes % 2 == 1 && length > 0 && matches!(bytes[end - length], b'n' | b'r' | b't') {
            length -= 1;
        }
        let word = &text[end - length..end];
        (length, literal_prefix(word, quote))
    };
    // `r#"…"#`: hashes before the quote, after a prefix with an `r` or after
    // no word at all (Swift's `#"…"#`, KT-2053 round 5: it closes only at
    // `"#`, else runs to the end).
    let hashes = if quote == b'"' {
        bytes[from..at]
            .iter()
            .rev()
            .take_while(|byte| **byte == b'#')
            .count()
    } else {
        0
    };
    let (raw_word, raw_prefix) = word_before(at - hashes);
    let raw = hashes > 0
        && (raw_word == 0
            || raw_prefix
                && text[at - hashes - raw_word..at - hashes]
                    .to_ascii_lowercase()
                    .contains('r'));
    let (hashes, (word, is_prefix)) = if raw {
        (hashes, (raw_word, raw_prefix))
    } else {
        (0, word_before(at))
    };
    let before = (at > from).then(|| bytes[at - 1]);
    // Right after an escaped backslash (`abc\\\"`), the quote ends a value too:
    // the reader takes `\x` in pairs, so a `\` before it is one of a pair.
    let closes = text[from..at].ends_with("***REDACTED***")
        || (!raw && word > 0 && !is_prefix)
        || (word == 0 && hashes == 0 && matches!(before, Some(b')' | b']' | b'}' | b'\\')));
    let closer = if quote == b'\'' && !escaped {
        Quote::Skip(at + 1)
    } else {
        Quote::Ends
    };
    if closes {
        return closer;
    }
    // A quote right after `.` `!` `?`: read below, once the literal is known.
    let sentence_end = word == 0 && hashes == 0 && matches!(before, Some(b'.' | b'!' | b'?'));
    if quote == b'\'' && !escaped && word == 0 {
        // A lifetime: `&'a`, `<'a>`, `, 'a` in a generic, a word not closed by
        // a `'` right after it (`'a'` is a char literal).
        // A generic's list may run across lines (`Foo<\n    'a,\n>`).
        let in_generic = open.last() == Some(&b'>')
            && text[from..at]
                .trim_end_matches([' ', '\t', '\n', '\r'])
                .ends_with([',', '<']);
        if matches!(before, Some(b'&' | b'<')) || in_generic {
            let name = bytes[at + 1..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
                .count();
            let starts = bytes
                .get(at + 1)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_');
            if starts && bytes.get(at + 1 + name) != Some(&b'\'') {
                return Quote::Skip(at + 1 + name);
            }
        }
    }
    let start = at + if escaped { 2 } else { 1 };
    let close = if raw {
        format!(
            "{}{}",
            if escaped { "\\\"" } else { "\"" },
            "#".repeat(hashes)
        )
    } else if escaped {
        "\\\"".to_string()
    } else {
        char::from(quote).to_string()
    };
    if sentence_end {
        // KT-2053 review: after `.` `!` `?` the quote may close the sentence
        // the expression is written in (`… and the key Y.", "b": "c"`) or open
        // the value (`$salt."…";`, `ok?"…":"x"`, `env.TOKEN??"…"`, `!!"…"`).
        // It closes when the literal it would open starts with a blank or a
        // punctuation that follows a closing quote, or when the quote that
        // would close that literal is right before a word (it really is the
        // next opener: `Y.", "b`); otherwise it opens the value.
        //
        // KT-2053 second security review: in an operator context the first
        // test is left out, as no sentence ends there. A `.` is an operator
        // after anything but a word (a blank, a closer, a quote: `f()."…"`,
        // `"a".."…"`; not an ellipsis `..."`) or after a word that is a
        // variable or a member (`$prefix.":"`, `$this->p.`, `self::P.`); a `?`
        // after a `?` (`??":…"`), after anything but a word or a blank, or
        // when the literal is followed by a ternary's `:` (`empty?" ":"…"`);
        // a `!` after anything but a word or a blank (`!!" …"`).
        let punct = bytes[at - 1];
        let previous = (at - 1 > from).then(|| bytes[at - 2]);
        let operator = match (punct, previous) {
            (_, None) => true,
            (b'.', Some(b'.')) => at - 2 == from || bytes[at - 3] != b'.',
            (b'.', Some(byte)) if word_byte(byte) => variable_before(bytes, from, at - 1),
            (b'.', Some(_)) => true,
            (b'?', Some(b'?')) => true,
            (_, Some(byte)) => !(word_byte(byte) || matches!(byte, b' ' | b'\t' | b'\n' | b'\r')),
        };
        let first = bytes.get(start).copied();
        let printed_blank =
            first == Some(b'\\') && matches!(bytes.get(start + 1), Some(b'n' | b'r' | b't'));
        let blank = match first {
            None => true,
            Some(byte) => {
                byte.is_ascii_whitespace()
                    || matches!(byte, b',' | b';' | b':' | b')' | b']' | b'}')
            }
        };
        let sentence = !operator && (blank || printed_blank);
        if sentence && punct != b'?' {
            return closer;
        }
        return match scan_body(text, start, quote, escaped, raw, &close, memo) {
            Quote::Literal { next, .. } if bytes.get(next).copied().is_some_and(word_byte) => {
                closer
            }
            Quote::Literal { next, .. } if sentence && !ternary_colon(bytes, next) => closer,
            Quote::Cut(_) if sentence => closer,
            found => found,
        };
    }
    scan_body(text, start, quote, escaped, raw, &close, memo)
}

/// Whether the word that ends right before `dot` (in the expression that
/// started at `from`) is a variable or a member: right after a `$` (PHP, Perl:
/// `$prefix.`), a `->` (`$this->prefix.`) or a `::` (`self::PREFIX.`).
fn variable_before(bytes: &[u8], from: usize, dot: usize) -> bool {
    let length = bytes[from..dot]
        .iter()
        .rev()
        .take_while(|byte| word_byte(**byte))
        .count();
    expression_work(length + 1);
    let head = &bytes[from..dot - length];
    head.ends_with(b"$") || head.ends_with(b"->") || head.ends_with(b"::")
}

/// Whether a ternary's `:` follows the literal that closed right before
/// `next`, past blanks (`empty?" ":"…"`); a `::` is a path.
fn ternary_colon(bytes: &[u8], next: usize) -> bool {
    let blanks = bytes[next.min(bytes.len())..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    expression_work(blanks + 1);
    bytes.get(next + blanks) == Some(&b':') && bytes.get(next + blanks + 1) != Some(&b':')
}

/// Where a scan by [`scan_body`] ran to the end of the text without meeting
/// the quote that closes its literal, by closing sequence (`"`, `'`, a
/// backtick, `\"`, or the `"#…` of a raw literal): the lowest body start each
/// was scanned from.
///
/// A scan of the same sequence from any later body start runs to the end too,
/// so it is answered at once: an input full of unclosed raw literals
/// (`token = r#"a token = r#"a …`, each name read again from inside the one
/// before it) stays linear. For a raw literal `find` is exact. For a quote the
/// scan takes `\x` in pairs, and a later body starts right after an opening
/// quote of the same sequence: the earlier scan met that quote escaped (or it
/// would have closed there), so it resumed exactly at that body start.
///
/// It also remembers the last line break found by [`CutMemo::line_break_after`],
/// what [`comment_run`] found after each line break it crossed, the last
/// match of each closing sequence [`CutMemo::find_after`] looked for, and the
/// last line start [`CutMemo::line_start`] found.
#[derive(Default)]
struct CutMemo(
    Vec<(String, usize)>,
    Option<(usize, usize)>,
    std::collections::HashMap<usize, Option<Next>>,
    Vec<(Vec<u8>, usize, Option<usize>)>,
    Option<(usize, usize)>,
);

impl CutMemo {
    /// Where the line that holds `at` starts: right past the last `\n` or `\r`
    /// before it, or 0 (KT-2053 second security review: a `\r`-only text has
    /// no `\n` to stop a search backwards). The last answer is remembered with
    /// the position it was asked at, so a later query scans forward from there
    /// only, and one between that line start and that position is answered at
    /// once: the pass asks at increasing positions, so it stays linear.
    fn line_start(&mut self, bytes: &[u8], at: usize) -> usize {
        let (scan_from, mut start) = match self.4 {
            Some((asked, start)) if at >= asked => (asked, start),
            Some((_, start)) if at >= start => return start,
            _ => {
                // Before anything asked: back to the previous line break.
                let back = bytes[..at]
                    .iter()
                    .rposition(|byte| matches!(byte, b'\n' | b'\r'))
                    .map_or(0, |index| index + 1);
                expression_work(at - back + 1);
                self.4 = Some((at, back));
                return back;
            }
        };
        for (offset, byte) in bytes[scan_from..at].iter().enumerate() {
            if matches!(byte, b'\n' | b'\r') {
                start = scan_from + offset + 1;
            }
        }
        expression_work(at - scan_from + 1);
        self.4 = Some((at, start));
        start
    }

    /// Where `close` first occurs at or after `at`, if it does. The last
    /// answer for each `close` is remembered with the position its search
    /// started from, so a query inside that range is answered at once and a
    /// query before it searches only up to it (as [`CutMemo::line_break_after`]):
    /// an input full of block comments that each look for the same `*/` stays
    /// linear.
    fn find_after(&mut self, bytes: &[u8], close: &[u8], at: usize) -> Option<usize> {
        let known = self.3.iter().position(|(sequence, ..)| sequence == close);
        let cached = known.map(|index| (self.3[index].1, self.3[index].2));
        if let Some((start, found)) = cached {
            if at >= start && found.is_none_or(|found| at <= found) {
                return found;
            }
        }
        let limit = cached.filter(|(start, _)| at < *start);
        let mut index = at;
        let found = loop {
            if let Some((start, found)) = limit {
                if index >= start {
                    break found;
                }
            }
            if index + close.len() > bytes.len() {
                break None;
            }
            if bytes[index..].starts_with(close) {
                break Some(index);
            }
            index += 1;
        };
        expression_work(index.saturating_sub(at) + close.len());
        let entry = (close.to_vec(), at, found);
        match known {
            Some(index) => self.3[index] = entry,
            None => self.3.push(entry),
        }
        found
    }

    fn runs_to_end(&self, close: &str, start: usize) -> bool {
        self.0
            .iter()
            .any(|(known, from)| known == close && *from <= start)
    }

    fn record(&mut self, close: &str, start: usize) {
        match self.0.iter_mut().find(|(known, _)| known == close) {
            Some((_, from)) => *from = (*from).min(start),
            None => self.0.push((close.to_string(), start)),
        }
    }

    /// The first line break at or after `at` (`\n`, `\r`, or a printed `\n` /
    /// `\r`), or the end of the text. The last answer is remembered as the
    /// range it holds for (no break between where its scan started and the
    /// break found), so a query inside that range is answered at once and a
    /// query before it scans only up to it: an input full of comment lines
    /// that each ask from inside the same long line stays linear.
    fn line_break_after(&mut self, bytes: &[u8], at: usize) -> usize {
        if let Some((start, found)) = self.1 {
            if (start..=found).contains(&at) {
                return found;
            }
        }
        let known = self.1.filter(|(start, _)| at < *start);
        let mut index = at;
        let found = loop {
            if let Some((start, found)) = known {
                if index >= start {
                    break found;
                }
            }
            match bytes.get(index) {
                None => break bytes.len(),
                Some(b'\n' | b'\r') => break index,
                Some(b'\\') if matches!(bytes.get(index + 1), Some(b'n' | b'r')) => break index,
                Some(_) => index += 1,
            }
        };
        expression_work(index.min(bytes.len()) - at.min(bytes.len()) + 1);
        self.1 = Some((at, found));
        found
    }
}

/// Counts the bytes the value-expression pass reads, so that a test can prove
/// it stays linear on an adversarial input. Nothing outside tests.
#[inline]
fn expression_work(bytes: usize) {
    #[cfg(test)]
    EXPRESSION_WORK.with(|work| work.set(work.get().saturating_add(bytes)));
    #[cfg(not(test))]
    let _ = bytes;
}

#[cfg(test)]
thread_local! {
    static EXPRESSION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reads the body of a literal that starts at `start` up to `close`, its
/// closing sequence (the `"#…` of a `raw` literal, else `quote`, escaped once
/// more in an `escaped` literal): a [`Quote::Literal`], or a [`Quote::Cut`] when the text
/// ends first or, in an escaped literal, a bare `"` ends the string around it.
fn scan_body(
    text: &str,
    start: usize,
    quote: u8,
    escaped: bool,
    raw: bool,
    close: &str,
    memo: &mut CutMemo,
) -> Quote {
    let bytes = text.as_bytes();
    if memo.runs_to_end(close, start) {
        return Quote::Cut(start..text.len());
    }
    if raw {
        return match text[start..].find(close) {
            Some(length) => {
                expression_work(length);
                Quote::Literal {
                    body: start..start + length,
                    next: start + length + close.len(),
                }
            }
            None => {
                expression_work(text.len() - start);
                memo.record(close, start);
                Quote::Cut(start..text.len())
            }
        };
    }
    let mut index = start;
    let found = loop {
        if index >= bytes.len() {
            break None;
        }
        match bytes[index] {
            b'\\' if escaped => match bytes.get(index + 1) {
                Some(b'"') => {
                    break Some(Quote::Literal {
                        body: start..index,
                        next: index + 2,
                    });
                }
                // `\\`: a backslash of the value, with the character it
                // escapes there, itself written plain or escaped.
                Some(b'\\') => {
                    index += 2;
                    match bytes.get(index) {
                        Some(b'\\') => index += 2,
                        Some(b'"') | None => {}
                        Some(_) => index += 1,
                    }
                }
                _ => index += 2,
            },
            b'\\' => index += 2,
            b'"' if escaped => break Some(Quote::Cut(start..index)),
            byte if byte == quote && !escaped => {
                break Some(Quote::Literal {
                    body: start..index,
                    next: index + 1,
                });
            }
            _ => index += 1,
        }
    };
    expression_work(index.min(bytes.len()) - start);
    found.unwrap_or_else(|| {
        memo.record(close, start);
        Quote::Cut(start..text.len())
    })
}

/// Masks every quoted literal in the value expression of each secret-ish name
/// ([`expression_literals`]) for which `masks(name, body)` holds, keeping its
/// quotes and prefix. A body already masked is left as is. Returns the text and
/// the number of literals masked.
///
/// An expression holds the expression of any name inside it (it ends at the
/// same terminator or later), so it is read once, and a literal is masked when
/// `masks` holds for its own name or a name written between them
/// (`pin: Some(\n token = b"…"`). A literal left unmasked that runs to the end
/// of the text does not hide what follows it: reading resumes inside it, and a
/// later literal running to the end there is judged on its text up to the next
/// name (each byte judged once, so the pass stays linear on any input).
///
/// A unified diff holds two versions of its lines ([`diff_versions`]); the
/// literals masked are those of the text as it is and of each version, so a
/// value continued by a changed line (` token = a ||\n-  "x";\n+  "…";`) is
/// read in the version where it goes on.
fn mask_value_expressions(text: &str, masks: &dyn Fn(&str, &str) -> bool) -> (String, usize) {
    // A version only blanks bytes: it has no `:` / `=` the text lacks.
    if !text.contains([':', '=']) {
        return (text.to_string(), 0);
    }
    let versions = diff_versions(text);
    let gated = value_expression_gate(text)
        || versions
            .iter()
            .flatten()
            .any(|version| value_expression_gate(version));
    if !gated {
        return (text.to_string(), 0);
    }
    mask_value_expressions_in(text, versions.as_ref(), masks)
}

/// [`mask_value_expressions`] without its gate.
fn mask_value_expressions_ungated(
    text: &str,
    masks: &dyn Fn(&str, &str) -> bool,
) -> (String, usize) {
    mask_value_expressions_in(text, diff_versions(text).as_ref(), masks)
}

/// The old and the new version of a unified diff (KT-2053 round 5), each as
/// long as `text`, byte for byte, so a range in either is a range in `text`:
/// in the old one every line that starts with `+` is blanked and the `-`
/// marker of every other line is a blank, in the new one the other way round.
/// The carry that enters a `-` run thus enters the `+` run after it, and a
/// context line after both continues from either. `None` when no line starts
/// with `+` or `-`. A line blanked whole keeps the text valid UTF-8.
fn diff_versions(text: &str) -> Option<[String; 2]> {
    let bytes = text.as_bytes();
    let marked = bytes
        .first()
        .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        || text.contains("\n+")
        || text.contains("\n-");
    if !marked {
        return None;
    }
    let (mut old, mut new) = (bytes.to_vec(), bytes.to_vec());
    let mut start = 0;
    while start < bytes.len() {
        let end = text[start..]
            .find('\n')
            .map_or(bytes.len(), |index| start + index);
        let (removed, kept) = match bytes[start] {
            b'+' => (&mut old, &mut new),
            b'-' => (&mut new, &mut old),
            _ => {
                start = end + 1;
                continue;
            }
        };
        removed[start..end].fill(b' ');
        kept[start] = b' ';
        start = end + 1;
    }
    expression_work(2 * bytes.len());
    Some([String::from_utf8(old).ok()?, String::from_utf8(new).ok()?])
}

/// Masks the literals [`value_expression_bodies`] finds in `text` and in each
/// of its diff `versions` (same offsets), overlapping ranges merged.
fn mask_value_expressions_in(
    text: &str,
    versions: Option<&[String; 2]>,
    masks: &dyn Fn(&str, &str) -> bool,
) -> (String, usize) {
    let mut bodies = value_expression_bodies(text, masks);
    for version in versions.into_iter().flatten() {
        bodies.extend(value_expression_bodies(version, masks));
    }
    // Overlapping bodies (a comment's literal its line does not close, the
    // same literal read in two versions) are masked as one.
    bodies.sort_by_key(|body| (body.start, body.end));
    let mut merged: Vec<std::ops::Range<usize>> = Vec::with_capacity(bodies.len());
    for body in bodies {
        match merged.last_mut() {
            Some(last) if body.start < last.end => last.end = last.end.max(body.end),
            _ => merged.push(body),
        }
    }
    let bodies = merged;
    if bodies.is_empty() {
        return (text.to_string(), 0);
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for body in &bodies {
        out.push_str(&text[copied..body.start]);
        out.push_str(VALUE_MASK);
        copied = body.end;
    }
    out.push_str(&text[copied..]);
    (out, bodies.len())
}

/// What [`mask_value_expressions`] writes in place of a literal's body.
const VALUE_MASK: &str = "***REDACTED***";

/// The bodies, in order, of the literals [`mask_value_expressions`] masks in
/// `text` (see there).
fn value_expression_bodies(
    text: &str,
    masks: &dyn Fn(&str, &str) -> bool,
) -> Vec<std::ops::Range<usize>> {
    const MASK: &str = VALUE_MASK;
    // Every name and where its `:` / `=` ends, in order.
    let names: Vec<(usize, usize, &str)> = VALUE_EXPRESSION_NAME
        .captures_iter(text)
        .filter_map(|captures| {
            let matched = captures.get(0)?;
            let name = captures.get(1)?;
            Some((matched.start(), matched.end(), name.as_str()))
        })
        .collect();
    let mut bodies = Vec::new();
    let mut from = 0;
    let mut index = 0;
    let mut memo = CutMemo::default();
    // Whether a literal running to the end of the text was already judged and
    // left: every later one is a suffix of it, inside the text read again.
    let mut tail_judged = false;
    while index < names.len() {
        let (start, separator_end, _) = names[index];
        if start < from {
            index += 1;
            continue;
        }
        let (literals, end) = expression_literals(text, separator_end, &mut memo);
        let mut resume = end.max(separator_end);
        // The names of the expression read so far (this one, then those inside
        // it), each once.
        let mut inside: Vec<String> = Vec::new();
        let mut next = index;
        for body in literals {
            while next < names.len() && names[next].1 <= body.start {
                let name = names[next].2.to_ascii_lowercase();
                if !inside.contains(&name) {
                    inside.push(name);
                }
                next += 1;
            }
            let value = &text[body.clone()];
            if value.is_empty() || value == MASK {
                continue;
            }
            let runs_to_end = body.end == text.len();
            // A literal running to the end inside the tail of one already left
            // is judged up to the next name, whose own expression reads the
            // rest: judging every suffix whole would be quadratic.
            let judged = if runs_to_end && tail_judged {
                let after = names.partition_point(|name| name.0 <= body.start);
                let until = names.get(after).map_or(body.end, |name| name.0);
                &text[body.start..until]
            } else {
                value
            };
            expression_work(judged.len().saturating_mul(inside.len()));
            if !inside.iter().any(|name| masks(name, judged)) {
                if runs_to_end {
                    resume = body.start;
                    tail_judged = true;
                }
                continue;
            }
            bodies.push(body);
        }
        from = resume;
        index += 1;
    }
    bodies
}

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
/// the first byte past the `:`/`=` that is not up to a space must be that quote;
/// a wrapped-value pattern reads the identifier of its call there, or the `|`
/// of a closure (KT-2053). A quoted-key
/// pattern has the key's closing quote (`key_quote`) right before the `:`.
fn assignment_gate(text: &str, opener: Opener, key_quote: Option<u8>) -> bool {
    let Some(tails) = ASSIGNMENT_TAILS.as_ref() else {
        return true;
    };
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(index, byte)| {
        if *byte != b':' && *byte != b'=' {
            return false;
        }
        let opens = bytes[index + 1..].iter().find(|byte| **byte > b' ');
        let opens_right = match opener {
            Opener::Any => true,
            Opener::Quote(quote) => opens == Some(&quote),
            Opener::Call => {
                opens.is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'|'))
            }
        };
        if !opens_right {
            return false;
        }
        let before = &bytes[..index];
        let end = before
            .iter()
            .rposition(|byte| *byte > b' ')
            .map_or(0, |last| last + 1);
        let mut before = &before[..end];
        if let Some(key_quote) = key_quote {
            // `NAME"` or `NAME\"`: the key's closing quote, escaped or not.
            let Some(unquoted) = before.strip_suffix(&[key_quote]) else {
                return false;
            };
            before = unquoted.strip_suffix(b"\\").unwrap_or(unquoted);
        }
        ends_with_secret_word(before, tails)
    })
}

/// What [`assignment_gate`] expects right after the `:`/`=`, past whitespace.
#[derive(Clone, Copy)]
enum Opener {
    /// Anything: the bare-value pattern.
    Any,
    /// This quote (or the `\` of `\"`).
    Quote(u8),
    /// An identifier opening the call that wraps the value (KT-2039), or the
    /// `|` of a closure (KT-2053).
    Call,
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
    redact_json_aware(input, redact_for_audit_artifact_text)
}

fn redact_for_audit_artifact_text(input: &str) -> (String, usize) {
    let (out, vendor_count) = apply_patterns(input, PATTERNS.as_slice(), &VENDOR_PREFILTER);
    let (out, expression_count) = mask_value_expressions(&out, &|_, _| true);
    let (out, assignment_count) =
        apply_patterns(&out, ASSIGNMENT.as_slice(), &ASSIGNMENT_PREFILTER);
    let count = vendor_count
        .saturating_add(expression_count)
        .saturating_add(assignment_count);
    if out == input {
        (out, 0)
    } else {
        (out, count.max(1))
    }
}

// A backslash ends the URL: in JSON nested in a string it escapes the closing
// quote, and the URL parser would rewrite it into a `/`.
static URL_IN_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s"'<>`\\]+"#).expect("static URL regex"));

/// An endpoint for a `Debug` print (KT-2055, KT-2057): its userinfo, query and
/// fragment masked. Endpoints are stored as typed (no URL check at save), so
/// the text is read as written, not parsed. Only a leading `scheme://` is kept
/// as is; from the first `?` or `#` the rest is masked (`?api_key=…`,
/// `#access_token=…`), and everything before the last `@` ahead of it is
/// userinfo (`user:pass@host`, `https:/u:p@host`, `https:\\u:p@host`). An `@`
/// past that `?` / `#` leaves it ambiguous (`?user=a@b&key=…`, or a `?` in a
/// password), so nothing after the scheme is printed. A path holding an `@` is
/// masked with it: a debug print may say less, never more.
pub fn mask_endpoint_secrets(url: &str) -> std::borrow::Cow<'_, str> {
    const MASK: &str = "***";
    let scheme_end = url
        .find("://")
        .filter(|end| {
            url[..*end].starts_with(|c: char| c.is_ascii_alphabetic())
                && url[..*end]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
        })
        .map_or(0, |end| end + 3);
    let query = url[scheme_end..].find(['?', '#']).map(|at| scheme_end + at);
    let userinfo_end = url.rfind('@');
    if userinfo_end.is_none() && query.is_none() {
        return std::borrow::Cow::Borrowed(url);
    }
    let mut masked = String::with_capacity(url.len());
    masked.push_str(&url[..scheme_end]);
    if userinfo_end
        .zip(query)
        .is_some_and(|(at, query)| at > query)
    {
        masked.push_str(MASK);
        return std::borrow::Cow::Owned(masked);
    }
    let host = match userinfo_end {
        Some(at) => {
            masked.push_str(MASK);
            masked.push('@');
            at + 1
        }
        None => scheme_end,
    };
    match query {
        Some(query) => {
            masked.push_str(&url[host..=query]);
            masked.push_str(MASK);
        }
        None => masked.push_str(&url[host..]),
    }
    std::borrow::Cow::Owned(masked)
}

/// Drops the query string and userinfo of every URL in `text`: both may
/// carry a credential, and a diagnostic only needs the endpoint.
pub fn strip_url_secrets(text: &str) -> String {
    URL_IN_TEXT
        .replace_all(text, |caps: &regex_lite::Captures<'_>| {
            let raw = &caps[0];
            match reqwest::Url::parse(raw) {
                // Nothing to drop: keep the bytes (`Url` would add a `/`), so
                // that a changed text means a secret was removed.
                Ok(url)
                    if url.query().is_none()
                        && url.username().is_empty()
                        && url.password().is_none() =>
                {
                    raw.to_string()
                }
                Ok(mut url) => {
                    url.set_query(None);
                    let _ = url.set_username("");
                    let _ = url.set_password(None);
                    url.to_string()
                }
                // Unparseable: keep only what precedes a query or userinfo.
                Err(_) => raw.split(['?', '@']).next().unwrap_or("").to_string(),
            }
        })
        .into_owned()
}

/// For stored text whose credentials are unknown (history, logs, run output
/// excerpts): URL queries and userinfo dropped, then the vendor and
/// secret-assignment heuristics. It cannot catch a bare secret by value.
pub fn redact_stored_text(text: &str) -> String {
    redact_json_aware(text, redact_stored_part).0
}

/// [`redact_stored_text`] on one text: the URL strip counts as a change, since
/// [`redact_json_aware`] keeps a string's rewrite only when it is reported.
fn redact_stored_part(text: &str) -> (String, usize) {
    let stripped = strip_url_secrets(text);
    let (out, count) = redact_for_audit_artifact_text(&stripped);
    if out == text {
        (out, 0)
    } else {
        (out, count.max(1))
    }
}

/// KT-1656 — redaction of what an **agent wrote**, which is stored, kept in the
/// transcript and sent back to the model on every later turn.
///
/// The vendor shapes ([`redact_secrets`]) and the secret-named assignments and
/// JSON keys of [`redact_for_audit_artifact`], but a secret-named assignment is
/// only masked when its value is a credential ([`value_is_credential`]): an
/// agent's reply is full of code and prose that names a token without holding
/// one (`let token = lexer.next_token();`, `password: required`,
/// `"token": "<your-token>"`, `Authorization: Bearer $TOKEN`), which the audit
/// pass would rewrite irreversibly. A printed credential file keeps being
/// masked (`"GITHUB_PERSONAL_ACCESS_TOKEN": "t0ken…"`, `GEMINI_API_KEY=…`).
pub fn redact_agent_output(input: &str) -> String {
    redact_json_aware_with(input, redact_agent_output_text, &|key, value| {
        value_is_credential(key, value, true)
    })
    .0
}

/// KT-1656 — the credentials of a **known shape** only, for rewriting history
/// that was stored unmasked: a vendor-prefixed token, a JWT, an `Authorization`
/// or `Bearer` value, a connection-string password, a `"password": "…"` field,
/// each left as written when it is a placeholder. No secret-named assignment or
/// JSON key is read, and no JSON key masks its value, so a message that only
/// names a token is never touched.
pub fn redact_known_shapes(input: &str) -> String {
    redact_json_aware_with(
        input,
        |text| {
            apply_patterns_guarded(
                text,
                PATTERNS.as_slice(),
                &VENDOR_PREFILTER,
                Some(&vendor_match_is_credential),
            )
        },
        &|_, _| false,
    )
    .0
}

fn redact_agent_output_text(input: &str) -> (String, usize) {
    let (out, vendor_count) = apply_patterns_guarded(
        input,
        PATTERNS.as_slice(),
        &VENDOR_PREFILTER,
        Some(&vendor_match_is_credential),
    );
    let (out, expression_count) =
        mask_value_expressions(&out, &|name, value| value_is_credential(name, value, true));
    let (out, assignment_count) = apply_patterns_guarded(
        &out,
        ASSIGNMENT.as_slice(),
        &ASSIGNMENT_PREFILTER,
        Some(&assignment_is_credential),
    );
    let count = vendor_count
        .saturating_add(expression_count)
        .saturating_add(assignment_count);
    if out == input {
        (out, 0)
    } else {
        (out, count.max(1))
    }
}

/// A value that stands for a secret without being one: nothing, a value already
/// masked, a template or an environment reference (`$TOKEN`, `${{ secrets.X }}`,
/// `<your-token>`, `secret://NAME`), the name of an environment variable
/// (`VOTRE_API_KEY`), an ellipsis (`kr-join-...`, `…`), a filler (`xxxxxxxx`,
/// `0000000000`), a placeholder word (`your-api-key`, `changeme`, `string`), a
/// text with no letter or digit in it (the `//!` that opens the next comment
/// line), or prose in a script that has no spaces (Chinese, Japanese).
fn is_placeholder(value: &str) -> bool {
    const PLACEHOLDERS: &[&str] = &[
        "your-",
        "your_",
        "xxx",
        "0000000000",
        "placeholder",
        "changeme",
        "change-me",
        "example",
        "redacted",
        "dummy",
        "<",
        "todo",
        "...",
        "secret://",
    ];
    const WORDS: &[&str] = &[
        "string",
        "null",
        "none",
        "nil",
        "true",
        "false",
        "undefined",
        "secret",
        "token",
        "password",
        "apikey",
        "api_key",
        "api-key",
        "required",
        "optional",
        "pass",
        "passwd",
        "pwd",
        "pw",
    ];
    // The quote or escape a match opens with (`\"{TOKEN}`) is not the value.
    let value = value.trim().trim_start_matches(['\\', '"', '\'', '`']);
    let first = value.chars().next();
    let lowered = value.to_ascii_lowercase();
    let env_name = value.contains('_')
        && value
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && first.is_some_and(|c| c.is_ascii_uppercase());
    value.is_empty()
        || value.starts_with(['$', '<', '{', '%', '@'])
        || value.starts_with("--")
        || value.contains("${")
        || value.contains("{{")
        || value.contains('…')
        || !value.chars().any(|c| c.is_ascii_alphanumeric())
        || value.chars().any(|c| !c.is_ascii() && c.is_alphabetic())
        || env_name
        || value.chars().all(|c| Some(c) == first)
        || PLACEHOLDERS.iter().any(|needle| lowered.contains(needle))
        || WORDS.contains(&lowered.as_str())
        || is_invite_token(value)
}

/// A room invitation (`kr-join-<32 hex>`, `db::discussion_sessions::create_invite_token`):
/// an agent posts it on purpose so that a peer can `disc_join` with it, and it
/// lives `INVITE_TTL_SECS` (10 minutes) and is stored only as a hash. Masking it
/// at insert would break the invitation it carries, so the stored-agent-output
/// guards leave it as written. The audit pass ([`redact_for_audit_artifact`],
/// [`redact_stored_text`]) is not guarded and still masks it: it runs on history
/// long after the expiry, where a leaked `kr-join-` token is better gone.
///
/// The exemption is the WHOLE value, exactly as `create_invite_token` writes it
/// (`kr-join-` and 32 lowercase hex characters), optionally inside one quote or
/// backtick on each side. A prefix put in front of a real secret, an invitation
/// embedded in a longer value, an uppercase or non-hex suffix are not one.
fn is_invite_token(value: &str) -> bool {
    const PREFIX: &str = "kr-join-";
    let value = value.trim();
    let value = value
        .strip_prefix(['"', '\'', '`'])
        .unwrap_or(value)
        .trim_end_matches(['"', '\'', '`']);
    value.strip_prefix(PREFIX).is_some_and(|hex| {
        hex.len() == 32 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Whether the value of a secret-named assignment (`password: …`,
/// `API_KEY=…`, `"token": "…"`; `name` is the secret-ish name) is a credential
/// rather than prose, code or a placeholder. Heuristic, and tuned to miss a
/// short bare value rather than to rewrite a sentence of code.
///
/// Never a credential: a placeholder ([`is_placeholder`]), a value with
/// Never a credential: a placeholder ([`is_placeholder`]), a value with
/// whitespace (prose), a dotted identifier (`disc.inviteTabToken`, an i18n key)
/// or a snake_case one (`test_does_not_fill_source_id`), a value under a `pin`
/// name that is not a number of 4 to 12 digits. A **quoted** value is
/// a credential from 8 characters, from 6 under a `password`-like name. A
/// **bare** one must look like a token (12 characters or more, letters and
/// digits, no trailing `-` / `_`) and not like code (a call, an access path, a
/// URL, a file path, an operator).
fn value_is_credential(name: &str, value: &str, quoted: bool) -> bool {
    let value = value.trim();
    let full_name = name.to_ascii_lowercase();
    // `SECRET_KEY` is a secret, `PIN_VALUE` a pin: the qualifier is not the word.
    let name = without_qualifier(full_name.as_bytes())
        .and_then(|stem| std::str::from_utf8(stem).ok())
        .map_or(full_name.clone(), str::to_owned);
    if is_placeholder(value) || value.chars().any(char::is_whitespace) {
        return false;
    }
    // No carve-out for a camelCase word, a path, the name repeated or a short
    // number: each is also what a real password, token or PIN can look like
    // (`"password": "CorrectHorseBatteryStaple"`, `admin_password =
    // "adminpassword"`, `PIN=4821`). Zero defect on secrets: an identifier masked
    // in a transcript is the accepted cost (KT-1656, round 5).
    let dotted_identifier = value.contains('.')
        && value.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
        });
    let snake_identifier = value.contains('_')
        && value
            .split('_')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase()));
    if dotted_identifier || snake_identifier {
        return false;
    }
    // A PIN is a number; the word "pin" before anything else is prose or code.
    if name.ends_with("pin") {
        return (4..=12).contains(&value.len()) && value.chars().all(|c| c.is_ascii_digit());
    }
    if quoted {
        const PASSWORDS: &[&str] = &[
            "password",
            "passwd",
            "pwd",
            "passphrase",
            "passcode",
            "secret",
        ];
        let floor = if PASSWORDS.iter().any(|word| name.ends_with(word)) {
            6
        } else {
            8
        };
        return value.chars().count() >= floor;
    }
    value.chars().count() >= 12
        && value.chars().any(|c| c.is_ascii_digit())
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && !value.ends_with(['-', '_'])
        && !value.contains([
            '(', ')', '[', ']', '{', '}', '<', '>', ';', ',', '|', '&', '*',
        ])
        && !value.contains("://")
        && !value.contains("::")
        && !value.contains("->")
        && !value.contains("=>")
        && !value.starts_with(['/', '~', '.'])
}

/// The name in the text that opens a match (`  "GEMINI_API_KEY": "`,
/// `export GITHUB_TOKEN=`, `password: `): the word before the separator.
fn name_before_value(prefix: &str) -> &str {
    let separators = |text: &str| -> usize {
        text.trim_end_matches(|c: char| {
            c.is_whitespace() || matches!(c, ':' | '=' | '"' | '\'' | '`' | '\\')
        })
        .len()
    };
    let mut prefix = &prefix[..separators(prefix)];
    // KT-2039: `api_key: Some(`, `token = Secret::new(`: the call that wraps the
    // value is not the name, nor are the calls around it (KT-2044:
    // `api_key: Some(Token(`). KT-2053: the calls are read as the `call` of
    // [`ASSIGNMENT`] reads them, from the end (`strdup (`,
    // `Secret::<String>::new(`, `unwrap_or_else(|| `, `Some(\n    \"`).
    loop {
        let opened = before_call_space(prefix);
        let before = match opened.strip_suffix('(') {
            Some(open) => before_call_path(open.trim_end_matches([' ', '\t'])),
            None => match before_closure(opened) {
                Some(before) => before,
                None => break,
            },
        };
        prefix = &before[..separators(before)];
    }
    let start = prefix
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
        .map_or(0, |at| at + 1);
    &prefix[start..]
}

/// Guard of the secret-assignment patterns: group 1 holds the name, group 2 is
/// the value, quoted when the character before it is a quote.
fn assignment_is_credential(captures: &regex_lite::Captures<'_>, text: &str) -> bool {
    let Some(value) = captures.get(2) else {
        return true;
    };
    let quoted = text[..value.start()]
        .chars()
        .next_back()
        .is_some_and(|before| matches!(before, '"' | '\'' | '`'));
    let name = captures
        .get(1)
        .map_or("", |prefix| name_before_value(prefix.as_str()));
    value_is_credential(name, value.as_str(), quoted)
}

/// Guard of [`PATTERNS`] for stored agent output. A vendor-prefixed token is
/// masked unless it is a placeholder (`xoxb-xxxxxxxxxxxx`); the shapes that read
/// a value next to a name (an `Authorization` header, a `"password": "…"` field,
/// the password of a connection string) leave a placeholder as written.
fn vendor_match_is_credential(captures: &regex_lite::Captures<'_>, _text: &str) -> bool {
    let matched = captures.get(0).map_or("", |m| m.as_str());
    let lowered = matched.to_ascii_lowercase();
    if lowered.starts_with("authorization") {
        let value = matched.rsplit(char::is_whitespace).next().unwrap_or("");
        !is_placeholder(value)
    } else if lowered.starts_with('"') {
        let prefix = captures.get(1).map_or("", |prefix| prefix.as_str());
        let value = matched
            .get(prefix.len()..matched.len().saturating_sub(1))
            .unwrap_or("");
        value_is_credential(name_before_value(prefix), value, true)
    } else if lowered.contains("://") {
        captures
            .get(3)
            .is_none_or(|password| !is_placeholder(password.as_str()))
    } else {
        // `Token <value>` / `Bearer <value>` in a log line: the exemption of an
        // invitation is the value word on its own, never a match that merely
        // contains one.
        let value = matched.rsplit(char::is_whitespace).next().unwrap_or("");
        !is_invite_token(value) && !is_placeholder(matched)
    }
}

/// Object keys whose string value is a credential, whatever it holds. Same
/// names as the `"password": "…"` pattern of [`RAW_PATTERNS`], which stops at
/// the first `"` and so cannot read a value that carries an escaped one.
const JSON_CREDENTIAL_KEYS: &[&str] = &[
    "password",
    "token",
    "api_key",
    "apiKey",
    "secret",
    "access_token",
    "refresh_token",
    "client_secret",
    "private_key",
];

/// Whether the value of this object key is a credential: one of
/// [`JSON_CREDENTIAL_KEYS`], or a name that ends in a secret-ish word of
/// [`ASSIGNMENT_NAMES`] (`GEMINI_API_KEY`, `GITHUB_PERSONAL_ACCESS_TOKEN`: the
/// env block of a CLI's `settings.json`, KT-1656), as the text patterns read it.
fn is_credential_key(key: &str) -> bool {
    if JSON_CREDENTIAL_KEYS.contains(&key) {
        return true;
    }
    ASSIGNMENT_TAILS
        .as_ref()
        .is_some_and(|tails| ends_with_secret_word(key.as_bytes(), tails))
}

/// Runs `redact` on `input`, one JSON string at a time when `input` is a JSON
/// document (an object or an array), on the whole text otherwise.
///
/// The text patterns read a string's *encoded* form, where a value is followed
/// by a backslash and a quote: a value class that takes the backslash for a
/// secret character leaves the closing quote unescaped and the document no
/// longer parses (KT-1526). Here each string is decoded, redacted as the text
/// it holds, and written back encoded, so a pattern never reads the escapes
/// around a secret. A string `redact` leaves alone keeps its original bytes,
/// as does everything between the strings.
///
/// An object key keeps its exact bytes when no secret pattern matches anywhere
/// in it, so a key that merely looks like an assignment (`mode=fast`) is never
/// renamed. When a pattern does match (a map keyed by `Bearer <token>` or
/// `see token=abc here`), the key is redacted like a string value: decoded,
/// redacted, re-encoded, and the document stays valid. Text that is not a JSON
/// document as a whole (NDJSON, an excerpt cut mid-document) takes `redact`
/// as it is: its patterns keep the escapes around a value themselves.
fn redact_json_aware(input: &str, redact: impl Fn(&str) -> (String, usize)) -> (String, usize) {
    redact_json_aware_with(input, redact, &|_, _| true)
}

/// [`redact_json_aware`], where `masks_value(key, value)` decides whether the
/// string value of a credential-named key is masked whatever it holds (`true`
/// for the callers that mask every such value; stored agent output keeps a
/// placeholder, and the known-shape pass never masks by key).
fn redact_json_aware_with(
    input: &str,
    redact: impl Fn(&str) -> (String, usize),
    masks_value: &dyn Fn(&str, &str) -> bool,
) -> (String, usize) {
    let trimmed = input.trim_start();
    let is_container = trimmed.starts_with('{') || trimmed.starts_with('[');
    if !is_container || serde_json::from_str::<serde::de::IgnoredAny>(input).is_err() {
        return redact(input);
    }

    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut count = 0usize;
    let mut copied = 0usize;
    // The key of the member whose value is read next: set by a string followed
    // by `:`, cleared by anything but whitespace and the `:` itself.
    let mut key: Option<String> = None;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                let start = index;
                index += 1;
                while index < bytes.len() && bytes[index] != b'"' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                let end = (index + 1).min(bytes.len());
                index = end;
                let Ok(decoded) = serde_json::from_str::<String>(&input[start..end]) else {
                    continue;
                };
                let is_key = input[end..].trim_start().starts_with(':');
                let (clean, changed) = if is_key {
                    let redacted = redact(&decoded);
                    key = Some(decoded);
                    redacted
                } else if key
                    .take()
                    .filter(|key| is_credential_key(key))
                    .is_some_and(|key| !decoded.is_empty() && masks_value(&key, &decoded))
                {
                    let masked = "***REDACTED***";
                    (masked.to_string(), usize::from(decoded != masked))
                } else {
                    redact(&decoded)
                };
                if changed > 0 {
                    out.push_str(&input[copied..start]);
                    out.push_str(&encode_json_string(&clean));
                    copied = end;
                    count = count.saturating_add(changed);
                }
            }
            byte if byte.is_ascii_whitespace() || byte == b':' => index += 1,
            _ => {
                key = None;
                index += 1;
            }
        }
    }
    if count == 0 {
        return (input.to_string(), 0);
    }
    out.push_str(&input[copied..]);
    (out, count)
}

fn encode_json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"***REDACTED***\"".to_string())
}

/// KT-2039 — a secret field as `Debug` prints it: `"***REDACTED***"` in place
/// of a value (an empty one stays `""`, so "cleared" still reads apart from
/// "set"), `None` and `Some(…)` kept, a map's keys kept and its values masked,
/// a list's length kept and each item masked.
/// Used by [`debug_without_secrets!`](crate::debug_without_secrets).
pub struct Masked<'a, T: ?Sized>(pub &'a T);

/// A value [`Masked`] knows how to print without its secret.
pub trait MaskedDebug {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
}

impl MaskedDebug for String {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.is_empty() {
            "\"\""
        } else {
            "\"***REDACTED***\""
        })
    }
}

impl<T: MaskedDebug> MaskedDebug for Option<T> {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            None => f.write_str("None"),
            Some(value) => f.debug_tuple("Some").field(&Masked(value)).finish(),
        }
    }
}

impl<K: std::fmt::Debug, V: MaskedDebug, S> MaskedDebug for std::collections::HashMap<K, V, S> {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|(key, value)| (key, Masked(value))))
            .finish()
    }
}

impl<K: std::fmt::Debug, V: MaskedDebug> MaskedDebug for std::collections::BTreeMap<K, V> {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|(key, value)| (key, Masked(value))))
            .finish()
    }
}

impl<T: MaskedDebug> MaskedDebug for Vec<T> {
    fn fmt_masked(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(Masked)).finish()
    }
}

impl<T: ?Sized + MaskedDebug> std::fmt::Debug for Masked<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt_masked(f)
    }
}

/// KT-2039 — `Debug` for a struct that holds a credential, printing every field
/// but its secrets ([`Masked`]): a failing test prints `{:?}` / `{:#?}` of what
/// it compared, and that output is stored and posted to rooms.
///
/// `Type { a, b; masked: secret }` names every field: one added later and left
/// out does not compile. `Type { a; masked: secret; .. }`, for a large struct,
/// prints only the named fields and elides the rest — never an unnamed secret.
#[macro_export]
macro_rules! debug_without_secrets {
    ($ty:ident { $($field:ident),* $(,)? ; masked: $($secret:ident),+ $(,)? }) => {
        impl ::std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let $ty { $($field,)* $($secret,)+ } = self;
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), $field))*
                    $(.field(stringify!($secret), &$crate::core::redact::Masked($secret)))+
                    .finish()
            }
        }
    };
    ($ty:ident { $($field:ident),* $(,)? ; masked: $($secret:ident),+ $(,)? ; .. }) => {
        impl ::std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let $ty { $($field,)* $($secret,)+ .. } = self;
                f.debug_struct(stringify!($ty))
                    $(.field(stringify!($field), $field))*
                    $(.field(stringify!($secret), &$crate::core::redact::Masked($secret)))+
                    .finish_non_exhaustive()
            }
        }
    };
}

/// What a TOML parse error says, fit for a log, a notice or an API response:
/// the kind of problem and its line and column, never the source excerpt that
/// `toml::de::Error`'s `Display` prints (the whole offending line, which may be
/// a key or a credential), nor a key name or a value (a serde message such as
/// `invalid type: string "sk-…"` carries one). `text` is the source the error
/// was raised on, used only to turn the byte span into a line and column.
pub fn toml_error_summary(text: &str, error: &toml::de::Error) -> String {
    let message = error.message();
    let kind = if message.starts_with("invalid type") || message.starts_with("invalid value") {
        "a value of the wrong type"
    } else if message.starts_with("missing field") {
        "a missing required setting"
    } else if message.starts_with("unknown field") || message.starts_with("unknown variant") {
        "an unknown setting or value"
    } else {
        "invalid TOML"
    };
    match error.span() {
        Some(span) => {
            let at = span.start.min(text.len());
            let before = &text.as_bytes()[..at];
            let line = before.iter().filter(|b| **b == b'\n').count() + 1;
            let column = before.iter().rev().take_while(|b| **b != b'\n').count() + 1;
            format!("{kind} at line {line}, column {column}")
        }
        None => kind.to_string(),
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    #[test]
    fn a_url_in_escaped_json_keeps_its_closing_quote() {
        let nested = r#"{\"avatar_url\": \"https://avatars.example.com/u/1\", \"next\": \"https://api.example.com/x?token=s3cr3t\"}"#;
        assert_eq!(
            strip_url_secrets(nested),
            r#"{\"avatar_url\": \"https://avatars.example.com/u/1\", \"next\": \"https://api.example.com/x\"}"#
        );
    }

    // ── KT-1526: JSON documents stay JSON ─────────────────────────────

    const TOKEN: &str = "s3cr3tTokenValue9876";

    /// A Jira-like document: the description holds a curl header example.
    fn jira_document(header: &str) -> String {
        let description = format!("Call it with: curl {header} https://x.example/api");
        serde_json::json!({
            "issues": [{"key": "KT-1", "fields": {"description": description, "n": 3}}],
            "total": 1
        })
        .to_string()
    }

    fn assert_masked_and_valid(out: &str, secret: &str) {
        let parsed: serde_json::Value =
            serde_json::from_str(out).unwrap_or_else(|e| panic!("invalid JSON ({e}): {out}"));
        assert!(!out.contains(secret), "secret survived: {out}");
        assert!(out.contains("REDACTED"), "nothing masked: {out}");
        let description = parsed["issues"][0]["fields"]["description"]
            .as_str()
            .expect("description stays a string");
        assert!(
            description.contains("curl -H "),
            "text around lost: {description}"
        );
        assert!(
            description.ends_with("https://x.example/api"),
            "{description}"
        );
        assert_eq!(parsed["total"], 1);
        assert_eq!(parsed["issues"][0]["fields"]["n"], 3);
    }

    fn headers() -> Vec<String> {
        vec![
            format!(r#"-H "X-Api-Token: {TOKEN}""#),
            format!(r#"-H "Authorization: Bearer {TOKEN}""#),
            format!(r#"-H "X-Auth-Secret: {TOKEN}""#),
            format!(r#"-H 'X-Api-Token: {TOKEN}'"#),
        ]
    }

    #[test]
    fn redact_secrets_keeps_a_json_document_valid() {
        for header in headers() {
            let doc = jira_document(&header);
            assert!(doc.contains(r#"\""#) || header.contains('\''), "{doc}");
            let out = redact_secrets(&doc);
            // The vendor pass has no bare-assignment pattern: only a header
            // shape it knows is masked, and the output stays valid either way.
            assert!(
                serde_json::from_str::<serde_json::Value>(&out).is_ok(),
                "{out}"
            );
            if header.contains("Authorization") {
                assert_masked_and_valid(&out, TOKEN);
            }
        }
    }

    #[test]
    fn redact_stored_text_keeps_a_json_document_valid_and_masks_the_token() {
        for header in headers() {
            let out = redact_stored_text(&jira_document(&header));
            assert_masked_and_valid(&out, TOKEN);
        }
    }

    #[test]
    fn redact_for_audit_artifact_keeps_a_json_document_valid_and_masks_the_token() {
        for header in headers() {
            let (out, count) = redact_for_audit_artifact(&jira_document(&header));
            assert!(count > 0);
            assert_masked_and_valid(&out, TOKEN);
        }
    }

    #[test]
    fn the_escaped_closing_quote_of_a_header_survives() {
        // The shape of the 31/10 incident, byte for byte.
        let doc = format!(r#"{{"d":"-H \"X-Api-Token: {TOKEN}\" done"}}"#);
        let out = redact_stored_text(&doc);
        assert_eq!(out, r#"{"d":"-H \"X-Api-Token: ***REDACTED***\" done"}"#);
    }

    #[test]
    fn a_json_document_with_nothing_to_hide_is_returned_byte_for_byte() {
        let doc = "{ \"a\" : \"caf\\u00e9\",\n  \"b\": [1, 2.50, null, \"x\"] }";
        assert_eq!(redact_stored_text(doc), doc);
        assert_eq!(redact_for_audit_artifact(doc), (doc.to_string(), 0));
        assert_eq!(redact_secrets(doc), doc);
    }

    #[test]
    fn untouched_parts_of_a_redacted_json_document_keep_their_bytes() {
        let doc = format!(r#"{{ "keep" : "café" , "d":"apikey={TOKEN}" }}"#);
        let out = redact_stored_text(&doc);
        assert!(out.starts_with(r#"{ "keep" : "café" , "d":"#), "{out}");
        assert!(!out.contains(TOKEN));
        assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
    }

    #[test]
    fn a_credential_key_masks_its_value_even_with_escaped_quotes() {
        let doc = r#"{"password":"a\"b\\c","user":"alice","nested":{"token":"x\"yz-secret"}}"#;
        let out = redact_stored_text(doc);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["password"], "***REDACTED***");
        assert_eq!(parsed["nested"]["token"], "***REDACTED***");
        assert_eq!(parsed["user"], "alice");
        assert!(!out.contains("yz-secret"));
        assert!(!out.contains("a\\\"b"));
    }

    #[test]
    fn json_redaction_is_idempotent() {
        let once = redact_stored_text(&jira_document(&headers()[0]));
        assert_eq!(redact_stored_text(&once), once);
    }

    #[test]
    fn plain_text_that_only_looks_like_json_still_gets_the_text_patterns() {
        let out = redact_stored_text(&format!("[INFO] apikey={TOKEN}"));
        assert_eq!(out, "[INFO] apikey=***REDACTED***");
    }

    #[test]
    fn a_page_dataset_payload_keeps_its_shape_and_loses_the_token() {
        // A dataset point: an object of rows, one string field quoting a header.
        let point = serde_json::json!({
            "tickets": [
                {"key": "KT-1", "note": format!("-H \"X-Api-Token: {TOKEN}\""), "points": 5},
                {"key": "KT-2", "note": "plain", "points": 8},
            ],
            "summary": {"owner": "ana", "api_key": TOKEN}
        })
        .to_string();
        let out = redact_stored_text(&point);
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(!out.contains(TOKEN), "{out}");
        assert_eq!(
            parsed["tickets"][1],
            serde_json::json!({"key": "KT-2", "note": "plain", "points": 8})
        );
        assert_eq!(parsed["tickets"][0]["points"], 5);
        assert_eq!(parsed["summary"]["owner"], "ana");
    }

    #[test]
    fn a_large_json_output_stays_valid_after_redaction() {
        let items: Vec<_> = (0..4_000)
            .map(|i| {
                serde_json::json!({
                    "key": format!("KT-{i}"),
                    "description": format!("curl -H \"X-Api-Token: {TOKEN}{i}\" -d '{{\"a\": 1}}'"),
                })
            })
            .collect();
        let doc = serde_json::to_string(&items).unwrap();
        let out = redact_stored_text(&doc);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.len(), 4_000);
        assert!(!out.contains(TOKEN));
    }

    // ── KT-1526: text that is not a JSON document as a whole ──────────
    //
    // NDJSON, an excerpt cut mid-document and a JSON string inside a log line
    // take the text patterns: they must keep every escape around a secret.

    type TextRedactor = (&'static str, Box<dyn Fn(&str) -> String>);

    /// Every entry point, by name.
    fn text_redactors() -> Vec<TextRedactor> {
        vec![
            ("redact_secrets", Box::new(redact_secrets)),
            ("redact_stored_text", Box::new(redact_stored_text)),
            (
                "redact_for_audit_artifact",
                Box::new(|text| redact_for_audit_artifact(text).0),
            ),
        ]
    }

    /// `input` is not one JSON document. After redaction the token is gone,
    /// something was masked, `survivors` are intact and a second pass changes
    /// nothing. `assignments_only` skips `redact_secrets`, which has no bare
    /// `NAME: value` pattern.
    fn assert_text_masked(input: &str, survivors: &[&str], assignments_only: bool) {
        assert!(
            serde_json::from_str::<serde_json::Value>(input).is_err(),
            "the sample must not be one JSON document: {input}"
        );
        for (name, redact) in text_redactors() {
            if assignments_only && name == "redact_secrets" {
                continue;
            }
            let out = redact(input);
            assert!(!out.contains(TOKEN), "{name}: secret survived: {out}");
            assert!(out.contains("REDACTED"), "{name}: nothing masked: {out}");
            for survivor in survivors {
                assert!(out.contains(survivor), "{name}: lost {survivor:?}: {out}");
            }
            assert_eq!(redact(&out), out, "{name}: not idempotent");
        }
    }

    #[test]
    fn the_incident_bytes_in_a_truncated_excerpt_keep_their_escapes() {
        // An excerpt cut inside the next record: not parseable.
        let header = format!(
            r#"{{"issues":[{{"description":"-H \"X-Api-Token: {TOKEN}\" https://x.example/api"}},{{"key":"KT-2","descr"#
        );
        assert_text_masked(
            &header,
            &[r#"X-Api-Token: ***REDACTED***\" https://x.example/api"#],
            true,
        );
        let authorization =
            format!(r#"{{"description":"curl -H \"Authorization: Bearer {TOKEN}\" -d x","next":"#);
        assert_text_masked(&authorization, &[r#"Bearer ***REDACTED***\" -d x"#], false);
    }

    #[test]
    fn the_incident_bytes_in_an_ndjson_line_keep_their_escapes() {
        let header = format!(
            "{{\"key\":\"KT-1\",\"d\":\"-H \\\"X-Api-Token: {TOKEN}\\\" ok\"}}\n{{\"key\":\"KT-2\",\"d\":\"plain\"}}\n"
        );
        assert_text_masked(
            &header,
            &[
                r#"X-Api-Token: ***REDACTED***\" ok"#,
                r#"{"key":"KT-2","d":"plain"}"#,
            ],
            true,
        );
        let authorization = format!(
            "{{\"d\":\"Authorization: Bearer {TOKEN}\"}}\n{{\"d\":\"Authorization: Basic {TOKEN}\\\" end\"}}\n"
        );
        assert_text_masked(
            &authorization,
            &[
                r#"Bearer ***REDACTED***"}"#,
                r#"Basic ***REDACTED***\" end"}"#,
            ],
            false,
        );
        // Every line of the NDJSON is still a JSON document.
        for (name, redact) in text_redactors() {
            for text in [&header, &authorization] {
                for line in redact(text).lines() {
                    assert!(
                        serde_json::from_str::<serde_json::Value>(line).is_ok(),
                        "{name}: {line}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_json_string_inside_a_log_line_keeps_its_escapes() {
        let line = format!(r#"[WARN] step out: "-H \"X-Api-Token: {TOKEN}\"" (truncated"#);
        assert_text_masked(&line, &[r#"X-Api-Token: ***REDACTED***\""#], true);
    }

    #[test]
    fn a_value_never_takes_the_backslash_that_escapes_a_quote() {
        for (input, expected) in [
            (
                format!(r#"x-api-token: {TOKEN}\" tail"#),
                r#"x-api-token: ***REDACTED***\" tail"#,
            ),
            // An escaped backslash belongs to the value, the escaped quote after it does not.
            (
                format!(r#"x-api-token: {TOKEN}\\\" tail"#),
                r#"x-api-token: ***REDACTED***\" tail"#,
            ),
            // `\\` then a raw quote: the quote ends the value.
            (
                format!(r#"x-api-token: {TOKEN}\\" tail"#),
                r#"x-api-token: ***REDACTED***" tail"#,
            ),
            // A value written between escaped quotes is masked whole.
            (
                format!(r#"x-api-token: \"{TOKEN}\" tail"#),
                r#"x-api-token: \"***REDACTED***\" tail"#,
            ),
            (
                format!(r#"x-api-token: \"two words {TOKEN}\" tail"#),
                r#"x-api-token: \"***REDACTED***\" tail"#,
            ),
            // Cut before the closing escaped quote, or right after a backslash.
            (
                format!(r#"x-api-token: \"{TOKEN}"#),
                r#"x-api-token: \"***REDACTED***"#,
            ),
            (
                format!(r#"x-api-token: {TOKEN}\"#),
                r#"x-api-token: ***REDACTED***\"#,
            ),
        ] {
            let out = redact_for_audit_artifact(&input).0;
            assert_eq!(out, expected, "from {input}");
            assert!(!out.contains(TOKEN), "secret survived: {out}");
        }
    }

    #[test]
    fn an_authorization_value_stops_before_a_quote_and_keeps_an_opening_escaped_one() {
        for (input, expected) in [
            (
                format!(r#"Authorization: Bearer {TOKEN}"}}"#),
                r#"Authorization: Bearer ***REDACTED***"}"#,
            ),
            (
                format!(r#"Authorization: Bearer \"{TOKEN}\" x"#),
                r#"Authorization: Bearer \"***REDACTED***\" x"#,
            ),
            (
                format!("Authorization: Bearer {TOKEN}\\\\\\\" x"),
                "Authorization: Bearer ***REDACTED***\\\" x",
            ),
        ] {
            let out = redact_secrets(&input);
            assert_eq!(out, expected, "from {input}");
            assert!(!out.contains(TOKEN), "secret survived: {out}");
        }
    }

    #[test]
    fn a_credential_field_value_with_an_escaped_quote_is_masked_whole() {
        // Two documents: the text pattern reads the first value.
        let ndjson = "{\"password\":\"a\\\"b-SECRET-tail\",\"user\":\"alice\"}\n{\"n\":1}\n";
        for (name, redact) in text_redactors() {
            let out = redact(ndjson);
            assert!(!out.contains("SECRET-tail"), "{name}: {out}");
            let first = out.lines().next().unwrap();
            let parsed: serde_json::Value = serde_json::from_str(first)
                .unwrap_or_else(|e| panic!("{name}: invalid line ({e}): {first}"));
            assert_eq!(parsed["password"], "***REDACTED***", "{name}");
            assert_eq!(parsed["user"], "alice", "{name}");
        }
        // Cut right after an escaped quote: the value runs to the end.
        let cut = "{\"password\":\"abc\\\"";
        assert_eq!(redact_secrets(cut), "{\"password\":\"***REDACTED***");
    }

    #[test]
    fn an_object_key_keeps_its_bytes_unless_a_secret_matches_inside_it() {
        let vendor = "sk-abcdefghijklmnopqrstuvwxyz1234";
        let doc = format!(
            r#"{{"mode=fast":"a","password":"x","Bearer {TOKEN}":1,"see {vendor} here":2,"say \"hi\"":3,"see token={TOKEN} here":4}}"#
        );
        for (name, redact) in text_redactors() {
            let out = redact(&doc);
            let parsed: serde_json::Value = serde_json::from_str(&out)
                .unwrap_or_else(|e| panic!("{name}: invalid JSON ({e}): {out}"));
            let keys: Vec<&String> = parsed.as_object().unwrap().keys().collect();
            // No pattern matches: the key keeps its exact bytes, even when it
            // is shaped like an assignment.
            for kept in ["mode=fast", "password", "say \"hi\""] {
                assert!(
                    keys.iter().any(|key| *key == kept),
                    "{name}: {kept}: {keys:?}"
                );
            }
            assert!(out.contains(r#""say \"hi\"":3"#), "{name}: {out}");
            // A secret that is only part of a key is masked, the key re-encoded.
            assert!(!out.contains(vendor), "{name}: {out}");
            assert!(
                keys.iter().any(|key| *key == "see sk-***REDACTED*** here"),
                "{name}: {keys:?}"
            );
            assert!(
                keys.iter().any(|key| *key == "Bearer ***REDACTED***"),
                "{name}: {keys:?}"
            );
            // `token=…` inside a key is a secret for the assignment pass only.
            let masked = keys
                .iter()
                .any(|key| *key == "see token=***REDACTED*** here");
            assert_eq!(masked, name != "redact_secrets", "{name}: {keys:?}");
            assert_eq!(
                !out.contains(TOKEN),
                name != "redact_secrets",
                "{name}: {out}"
            );
        }
    }

    // ── KT-1673: second pass on KT-1526 ───────────────────────────────

    const URL_SECRETS: [&str; 3] = ["Qk7SecretValue9", "s3cr3t@", "user:s3cr3t"];

    #[test]
    fn a_json_document_loses_the_url_secrets_of_its_strings() {
        let doc = r#"{"next":"https://api.example.com/v1/items?api_key=Qk7SecretValue9","webhook":"https://user:s3cr3t@hooks.example.com/cb","n":2}"#;
        let out = redact_stored_text(doc);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect(&out);
        assert_eq!(parsed["next"], "https://api.example.com/v1/items");
        assert_eq!(parsed["webhook"], "https://hooks.example.com/cb");
        assert_eq!(parsed["n"], 2);
        for secret in URL_SECRETS {
            assert!(!out.contains(secret), "{secret}: {out}");
        }
        assert_eq!(redact_stored_text(&out), out, "not idempotent");
    }

    #[test]
    fn a_json_document_loses_the_url_secrets_of_its_keys() {
        let doc = r#"{"https://u:s3cr3t@x.example/p?secret=Qk7SecretValue9":1,"b":["https://x.example/a?token=Qk7SecretValue9"]}"#;
        let out = redact_stored_text(doc);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect(&out);
        assert_eq!(parsed["https://x.example/p"], 1);
        assert_eq!(parsed["b"][0], "https://x.example/a");
        for secret in URL_SECRETS {
            assert!(!out.contains(secret), "{secret}: {out}");
        }
    }

    #[test]
    fn a_json_url_with_nothing_to_drop_keeps_its_bytes() {
        let doc = r#"{"a":"https://x.example","b":"see https://x.example/p#frag now"}"#;
        assert_eq!(redact_stored_text(doc), doc);
    }

    #[test]
    fn a_truncated_json_credential_is_masked_to_the_end_of_the_excerpt() {
        for (input, expected) in [
            // Interior escaped quote, no closing quote.
            (
                r#"{"password":"prefix\"s3cr3t_tail"#,
                r#"{"password":"***REDACTED***"#,
            ),
            // No escape, no closing quote.
            (
                r#"{"password":"s3cr3t_tail"#,
                r#"{"password":"***REDACTED***"#,
            ),
            // Cut right after a lone backslash.
            (r#"{"token":"s3cr3t_tail\"#, r#"{"token":"***REDACTED***"#),
            // Cut right after an escaped quote.
            (r#"{"api_key":"s3cr3t\""#, r#"{"api_key":"***REDACTED***"#),
            // Interior escaped quote, closing quote present.
            (
                r#"{"password":"prefix\"s3cr3t_tail","user":"alice"#,
                r#"{"password":"***REDACTED***","user":"alice"#,
            ),
            // Several escapes, then the closing quote.
            (
                r#"{"secret":"a\\\"b\\s3cr3t_tail","user":"alice"#,
                r#"{"secret":"***REDACTED***","user":"alice"#,
            ),
        ] {
            assert!(serde_json::from_str::<serde_json::Value>(input).is_err());
            for (name, redact) in text_redactors() {
                let out = redact(input);
                assert_eq!(out, expected, "{name}: from {input}");
                assert!(!out.contains("s3cr3t"), "{name}: {out}");
                assert_eq!(redact(&out), out, "{name}: not idempotent");
            }
        }
    }

    #[test]
    fn an_unquoted_value_with_an_escaped_quote_or_backtick_is_masked_whole() {
        for input in [
            r"password=head\'s3cr3tTail",
            r"password=head\`s3cr3tTail",
            r#"password=head\"s3cr3tTail"#,
            r"x-api-token: head\'s3cr3tTail",
            r"secret=\'s3cr3tTail",
            r"password=s3cr3tTail\'",
        ] {
            for (name, redact) in text_redactors() {
                if name == "redact_secrets" {
                    continue;
                }
                let out = redact(input);
                assert!(!out.contains("s3cr3t"), "{name}: {out}");
                assert!(out.contains("***REDACTED***"), "{name}: {out}");
                assert_eq!(redact(&out), out, "{name}: not idempotent");
            }
        }
        for input in [
            r#"Authorization: Bearer head\"s3cr3tTail"#,
            r"Authorization: Bearer head\'s3cr3tTail",
            r"Authorization: Bearer head\`s3cr3tTail",
        ] {
            for (name, redact) in text_redactors() {
                let out = redact(input);
                assert!(!out.contains("s3cr3t"), "{name}: {out}");
                assert_eq!(redact(&out), out, "{name}: not idempotent");
            }
        }
        // The closing escaped quote of a JSON string still ends the value.
        let closed = r#"x-api-token: head\'s3cr3tTail\" tail"#;
        assert_eq!(
            redact_for_audit_artifact(closed).0,
            r#"x-api-token: ***REDACTED***\" tail"#
        );
    }

    #[test]
    fn a_quoted_assignment_cut_before_its_closing_quote_is_masked_to_the_end() {
        for (input, expected) in [
            (
                r#"client_secret: "my long s3cr3t phrase"#,
                r#"client_secret: "***REDACTED***"#,
            ),
            (
                "client_secret: 'my long s3cr3t phrase",
                "client_secret: '***REDACTED***",
            ),
            (
                "client_secret: `my long s3cr3t phrase",
                "client_secret: `***REDACTED***",
            ),
            (
                r#"x-api-token: \"two words s3cr3t"#,
                r#"x-api-token: \"***REDACTED***"#,
            ),
            // The cut text may end on a lone backslash or an escaped quote.
            (
                r#"password = "two words s3cr3t\"#,
                r#"password = "***REDACTED***"#,
            ),
            (
                r#"password = "two words s3cr3t\""#,
                r#"password = "***REDACTED***"#,
            ),
            // Several lines: nothing is left after the opening quote.
            (
                "secret: \"line one s3cr3t\nline two s3cr3t",
                "secret: \"***REDACTED***",
            ),
        ] {
            for (name, redact) in text_redactors() {
                if name == "redact_secrets" {
                    continue;
                }
                let out = redact(input);
                assert_eq!(out, expected, "{name}: {input}");
                assert!(!out.contains("s3cr3t"), "{name}: {out}");
                assert_eq!(redact(&out), out, "{name}: not idempotent");
            }
        }
        // A closing quote that exists is kept: the text after it stays.
        for (input, expected) in [
            (
                r#"client_secret: "my s3cr3t phrase" and more"#,
                r#"client_secret: "***REDACTED***" and more"#,
            ),
            (
                "client_secret: 'my s3cr3t phrase' and more",
                "client_secret: '***REDACTED***' and more",
            ),
            (
                r#"x-api-token: \"two words s3cr3t\" ok"#,
                r#"x-api-token: \"***REDACTED***\" ok"#,
            ),
            // The escaped form cut inside a JSON string: its own bare closing
            // quote stays, so the document keeps parsing.
            (
                r#"{"note":"x-api-token: \"two words s3cr3t"}"#,
                r#"{"note":"x-api-token: \"***REDACTED***"}"#,
            ),
        ] {
            let out = redact_for_audit_artifact(input).0;
            assert_eq!(out, expected, "{input}");
            assert_eq!(redact_for_audit_artifact(&out).0, out, "not idempotent");
        }
    }

    #[test]
    fn a_json_document_keeps_parsing_when_a_value_ends_on_an_escaped_quote() {
        // Reviewer scenarios, pinned: valid JSON in, valid JSON out, no leak.
        let document = r#"{"password":"secret\"","user":"alice"}"#;
        for (name, redact) in text_redactors() {
            let out = redact(document);
            assert_eq!(
                out, r#"{"password":"***REDACTED***","user":"alice"}"#,
                "{name}"
            );
            assert!(serde_json::from_str::<serde_json::Value>(&out).is_ok());
        }
        let headers = r#"x-api-token: \"head\"s3cr3tTail\" ok"#;
        for (name, redact) in text_redactors() {
            if name == "redact_secrets" {
                continue;
            }
            let out = redact(headers);
            assert_eq!(out, r#"x-api-token: \"***REDACTED***\" ok"#, "{name}");
        }
        for (input, expected) in [
            (r#"{"password":"\"#, r#"{"password":"***REDACTED***"#),
            (r#"{"password":"\""#, r#"{"password":"***REDACTED***"#),
        ] {
            for (name, redact) in text_redactors() {
                assert_eq!(redact(input), expected, "{name}: {input}");
            }
        }
    }

    #[test]
    fn a_closed_quoted_value_holding_an_escaped_newline_is_masked_whole() {
        // `\` + newline is an escape pair: `.` does not match the newline, so the
        // closed shapes must read the escaped character as `[\s\S]`.
        for (input, expected) in [
            (
                "secret: \"part1\\\npart2 s3cr3t\" ok",
                "secret: \"***REDACTED***\" ok",
            ),
            (
                "secret: 'part1\\\npart2 s3cr3t' ok",
                "secret: '***REDACTED***' ok",
            ),
            (
                "secret: `part1\\\npart2 s3cr3t` ok",
                "secret: `***REDACTED***` ok",
            ),
            (
                "{\"secret\": \"part1\\\npart2 s3cr3t\", \"user\": \"alice\"}",
                "{\"secret\": \"***REDACTED***\", \"user\": \"alice\"}",
            ),
            (
                "{\"client_secret\": \"part1\\\r\npart2 s3cr3t\"} tail",
                "{\"client_secret\": \"***REDACTED***\"} tail",
            ),
        ] {
            for (name, redact) in text_redactors() {
                if name == "redact_secrets" {
                    continue;
                }
                let out = redact(input);
                assert_eq!(out, expected, "{name}: {input:?}");
                assert!(!out.contains("s3cr3t") && !out.contains("part1"), "{name}");
                assert_eq!(redact(&out), out, "{name}: not idempotent");
            }
        }
    }

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

    // ── KT-1656: a printed credential file is masked ───────────────────

    /// What `cat ~/.gemini/settings.json` prints: an env block of MCP servers
    /// keyed by the variable name, no vendor prefix on the values.
    const GEMINI_SETTINGS: &str = r#"{
  "security": { "auth": { "selectedType": "oauth-personal" } },
  "mcpServers": {
    "github": {
      "command": "npx",
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "t0kenValueNotShapedLikeAVendor1",
        "GEMINI_API_KEY": "plainKeyValue-8f3a9c2e7d41",
        "LOG_LEVEL": "debug"
      },
      "headers": { "X-Api-Key": "headerKeyValue-77aa11bb22cc" }
    }
  }
}"#;

    const GEMINI_SECRETS: [&str; 3] = [
        "t0kenValueNotShapedLikeAVendor1",
        "plainKeyValue-8f3a9c2e7d41",
        "headerKeyValue-77aa11bb22cc",
    ];

    fn assert_masked(out: &str) {
        for secret in GEMINI_SECRETS {
            assert!(!out.contains(secret), "{secret} survived: {out}");
        }
        assert!(out.contains("\"LOG_LEVEL\": \"debug\""), "{out}");
    }

    #[test]
    fn a_settings_json_printed_as_a_document_is_masked() {
        let (out, count) = redact_for_audit_artifact(GEMINI_SETTINGS);
        assert_masked(&out);
        assert!(count > 0);
        serde_json::from_str::<serde_json::Value>(&out).expect("still JSON");
        assert_eq!(redact_for_audit_artifact(&out).1, 0, "idempotent");
        assert_masked(&redact_stored_text(GEMINI_SETTINGS));
    }

    #[test]
    fn a_settings_json_quoted_in_a_message_is_masked() {
        let message = format!("Here is the file:\n```json\n{GEMINI_SETTINGS}\n```\nDone.");
        assert_masked(&redact_for_audit_artifact(&message).0);
        assert_masked(&redact_stored_text(&message));
    }

    #[test]
    fn a_settings_json_nested_in_a_json_string_is_masked() {
        // A tool result carried as an escaped string inside a JSON envelope.
        let envelope =
            serde_json::to_string(&serde_json::json!({ "output": GEMINI_SETTINGS })).unwrap();
        let (out, _) = redact_for_audit_artifact(&envelope);
        serde_json::from_str::<serde_json::Value>(&out).expect("still JSON");
        for secret in GEMINI_SECRETS {
            assert!(!out.contains(secret), "{secret} survived: {out}");
        }
        // The same text cut mid-document, which the JSON parser rejects.
        let cut = &envelope[..envelope.len() - 2];
        for secret in GEMINI_SECRETS {
            assert!(
                !redact_for_audit_artifact(cut).0.contains(secret),
                "{secret}"
            );
        }
    }

    // ── KT-1656: the shapes of the other agents' login files ───────────

    const GOOGLE_ACCESS: &str = "ya29.a0AfH6SMBx3Zk9QwErTyUiOpAsDfGhJkLzXcVbNm12345";
    const GOOGLE_REFRESH: &str =
        "1//0gAbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCdEfGhIjKlMnOpQrStUv";
    const GITHUB_FINE_GRAINED: &str =
        "github_pat_11ABCDEFG0abcdefghijkl_0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijk";
    const AWS_STS: &str = "ASIAY34FZKBOKMUTVV7A";

    fn login_shapes() -> [&'static str; 4] {
        [GOOGLE_ACCESS, GOOGLE_REFRESH, GITHUB_FINE_GRAINED, AWS_STS]
    }

    #[test]
    fn google_oauth_fine_grained_github_and_sts_tokens_are_masked_bare() {
        for secret in login_shapes() {
            let text = format!("the token is {secret} and that is all");
            for out in [
                redact_secrets(&text),
                redact_agent_output(&text),
                redact_known_shapes(&text),
            ] {
                assert!(!out.contains(secret), "{secret} survived: {out}");
                assert!(out.contains("***REDACTED***"), "{out}");
                assert!(out.starts_with("the token is "), "{out}");
                assert!(out.ends_with(" and that is all"), "{out}");
            }
            assert!(looks_like_secret(&text), "{secret}");
        }
    }

    #[test]
    fn an_oauth_creds_file_is_masked_with_and_without_its_keys() {
        // `~/.gemini/oauth_creds.json` as printed, and the same values under
        // names no key pattern reads.
        let creds = format!(
            r#"{{"access_token": "{GOOGLE_ACCESS}", "refresh_token": "{GOOGLE_REFRESH}", "scope": "openid", "token_type": "Bearer", "expiry_date": 1760000000000}}"#
        );
        let renamed = format!(
            r#"{{"a": "{GOOGLE_ACCESS}", "b": "{GOOGLE_REFRESH}", "c": "{GITHUB_FINE_GRAINED}", "d": "{AWS_STS}"}}"#
        );
        for text in [creds, renamed] {
            for out in [
                redact_agent_output(&text),
                redact_known_shapes(&text),
                redact_secrets(&text),
            ] {
                for secret in login_shapes() {
                    assert!(!out.contains(secret), "{secret} survived: {out}");
                }
                serde_json::from_str::<serde_json::Value>(&out).expect("still JSON");
            }
        }
    }

    #[test]
    fn the_new_shapes_leave_ordinary_text_alone() {
        for text in [
            "see ya29 and ya29.x",
            "the 1//0 route and 1//a/b/c",
            "github_pat_ is the prefix, github_pat_short too",
            "ASIA, ASIAN and ASIAPACIFICREGION are words",
            "ASIA0123456789ABCDEFG is one char too long to be a key",
            "https://example.com/1//0/short",
        ] {
            assert_eq!(redact_agent_output(text), text, "{text}");
            assert_eq!(redact_known_shapes(text), text, "{text}");
        }
        // A placeholder with a vendor shape stays as written in agent output.
        let placeholder = "ya29.xxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        assert_eq!(redact_agent_output(placeholder), placeholder);
    }

    #[test]
    fn a_secret_word_followed_by_a_qualifier_still_names_a_secret() {
        for text in [
            "SECRET_KEY=django-insecure-8f3a9c2e7d41b6",
            "export TOKEN_VALUE=8f3a9c2e7d41b6a0",
            "secret-key: 'plainSecretValue1'",
            "TOKEN_VALUE = \"plainTokenValue1\"",
            "passwordString=\"hunter22\"",
            "SECRET_VALUE: s3cr3tValue-0042",
        ] {
            let out = redact_agent_output(text);
            assert!(out.contains("***REDACTED***"), "{text} -> {out}");
            assert_eq!(redact_agent_output(&out), out, "idempotent: {out}");
            let audit = redact_for_audit_artifact(text).0;
            assert!(audit.contains("***REDACTED***"), "{text} -> {audit}");
        }
        let env_block = r#"{"env": {"TOKEN_VALUE": "plainTokenValue1", "SECRET_KEY": "plainSecretKey-42", "LOG_LEVEL": "debug"}}"#;
        let out = redact_agent_output(env_block);
        assert!(!out.contains("plainTokenValue1"), "{out}");
        assert!(!out.contains("plainSecretKey-42"), "{out}");
        assert!(out.contains("\"LOG_LEVEL\": \"debug\""), "{out}");
        serde_json::from_str::<serde_json::Value>(&out).expect("still JSON");
        assert!(!redact_known_shapes(env_block).contains("***REDACTED***"));
    }

    #[test]
    fn a_qualifier_does_not_turn_code_or_prose_into_a_secret() {
        for text in [
            "SECRET_KEY: String,",
            "let token_value: u64 = 0;",
            "token_value = compute(input)",
            "secret_key = os.environ[\"SECRET_KEY\"]",
            "\"secret_key\": \"<your key>\"",
            "TOKEN_VALUE=$TOKEN_VALUE",
            "tokenizer_key=abcdefghijkl1234",
            "monkey_value=abcdefghijkl1234",
            "the secret key is rotated yearly",
        ] {
            assert_eq!(redact_agent_output(text), text, "{text}");
        }
    }

    #[test]
    fn a_run_of_separators_before_the_qualifier_still_names_a_secret() {
        for text in [
            "SECRET__KEY=django-insecure-8f3a9c2e7d41b6",
            "secret--value: 's3cr3tValue-0042'",
            "export TOKEN___VALUE=8f3a9c2e7d41b6a0",
            "api_key__string = \"plainKeyString-7\"",
        ] {
            let out = redact_agent_output(text);
            assert!(out.contains("***REDACTED***"), "{text} -> {out}");
            assert!(
                redact_for_audit_artifact(text).0.contains("***REDACTED***"),
                "{text}"
            );
        }
        let env_block = r#"{"env": {"SECRET__KEY": "plainSecretKey-42", "TOKEN--VALUE": "plainTokenValue1", "LOG_LEVEL": "debug"}}"#;
        let out = redact_agent_output(env_block);
        assert!(!out.contains("plainSecretKey-42"), "{out}");
        assert!(!out.contains("plainTokenValue1"), "{out}");
        assert!(out.contains("\"LOG_LEVEL\": \"debug\""), "{out}");
        serde_json::from_str::<serde_json::Value>(&out).expect("still JSON");
        // The gate of the patterns reads the same names.
        assert!(is_credential_key("SECRET__KEY"));
        assert!(is_credential_key("secret--value"));
        assert!(!is_credential_key("__key"));
        assert_eq!(without_qualifier(b"secret__key"), Some(&b"secret"[..]));
        assert_eq!(without_qualifier(b"__value"), None);
    }

    /// KT-1656 — a room invitation is posted on purpose, so a peer can join: it is
    /// stored as written. A credential next to it is still masked.
    #[test]
    fn a_room_invitation_is_stored_as_written_and_a_credential_beside_it_is_not() {
        let invite = "kr-join-0123456789abcdef0123456789abcdef";
        for text in [
            format!("disc_join({{token: \"{invite}\"}})"),
            format!("disc_join({{\"token\": \"{invite}\"}})"),
            format!("{{\"token\":\"{invite}\"}}"),
            format!("token={invite}"),
            format!("Token {invite} is valid for ten minutes"),
        ] {
            assert_eq!(redact_agent_output(&text), text, "{text}");
        }
        let both = format!(
            "disc_join({{token: \"{invite}\"}}) and GEMINI_API_KEY=t0kenNotShapedLikeAVendor01"
        );
        let out = redact_agent_output(&both);
        assert!(out.contains(invite), "{out}");
        assert!(!out.contains("t0kenNotShapedLikeAVendor01"), "{out}");
        // A shorter `kr-join-` is not an invitation.
        assert!(redact_agent_output("token: \"kr-join-9f8e7d6c5b4a\"").contains("REDACTED"));
        // The exemption is the whole value, in the exact shape of
        // `create_invite_token`: 32 lowercase hex characters, nothing around them.
        let upper = "kr-join-0123456789ABCDEF0123456789ABCDEF";
        let non_hex = "kr-join-0123456789abcdefghijklmnopqrstuv";
        let longer = format!("{invite}0123456789abcdef");
        let one_short = "kr-join-0123456789abcdef0123456789abcde";
        for (text, secret) in [
            // A prefix attached to a real secret.
            (
                "API_KEY=\"kr-join-MyRealSecretValue0123456789abcdef\"".to_owned(),
                "MyRealSecretValue0123456789abcdef",
            ),
            (
                format!("API_KEY=\"{invite}MyRealSecret0123456789\""),
                "MyRealSecret0123456789",
            ),
            // An invitation embedded in a longer value.
            (format!("password: \"hunter2-{invite}-tail\""), "hunter2-"),
            (format!("token: \"prefix{invite}\""), "prefix"),
            (format!("token: \"{longer}\""), "0123456789abcdef\""),
            // Uppercase, non-hex and wrong-length suffixes.
            (format!("token: \"{upper}\""), upper),
            (format!("token: \"{non_hex}\""), non_hex),
            (format!("token: \"{one_short}\""), one_short),
            // The bare `Token <value>` of a log line follows the same rule.
            (format!("Token {longer} is valid"), longer.as_str()),
            (format!("Bearer {upper}"), upper),
        ] {
            let out = redact_agent_output(&text);
            assert!(out.contains("REDACTED"), "{text} -> {out}");
            assert!(!out.contains(secret), "{text} -> {out}");
        }
        assert!(is_invite_token(invite));
        assert!(is_invite_token(&format!("`{invite}`")));
        assert!(is_invite_token(&format!("\"{invite}\"")));
        assert!(!is_invite_token(&format!("x{invite}")));
        assert!(!is_invite_token(&format!("{invite}x")));
        assert!(!is_invite_token(upper));
        assert!(!is_invite_token(&longer));
        assert!(!is_invite_token(one_short));
        // The audit pass is not guarded: history is scrubbed of it.
        assert!(redact_for_audit_artifact(&format!("token: \"{invite}\""))
            .0
            .contains("REDACTED"));
    }

    /// KT-1656 round 5 — EXPECTED-MASKED. These are the "false positives" of the
    /// real-message measurement (`"token":"initGeneration"`, ``cancel token:
    /// `session/cancel` ``, `TooLarge`, `CHANNEL_TOKEN = 'channeltoken'`,
    /// `PIN=1234`). Round 4 stored them as written; a camelCase word, a path, the
    /// name repeated and a short number are also what a real password, token or
    /// PIN looks like, so the carve-outs leaked credentials. Zero defect on
    /// secrets: the identifier is masked, which is the accepted cost.
    #[test]
    fn an_identifier_looking_value_under_a_secret_name_is_masked() {
        for (text, value) in [
            (r#"{"token":"initGeneration"}"#, "initGeneration"),
            (r#""token": "TooLarge""#, "TooLarge"),
            ("cancel token: `session/cancel`", "session/cancel"),
            ("token: 'session/cancel'", "session/cancel"),
            ("CHANNEL_TOKEN = 'channeltoken'", "channeltoken"),
            ("const CHANNEL_TOKEN = \"channel-token\";", "channel-token"),
            ("PIN=1234", "1234"),
            ("pin: 123456", "123456"),
            ("\"pin\": \"0042\"", "0042"),
            ("password: \"1234567\"", "1234567"),
        ] {
            let out = redact_agent_output(text);
            assert!(out.contains("REDACTED"), "{text} -> {out}");
            assert!(!out.contains(value), "{text} -> {out}");
        }
    }

    /// KT-1656 round 5 — every leak the two security passes of round 4 listed.
    #[test]
    fn a_pin_password_or_token_that_looks_like_an_identifier_is_masked() {
        for (text, secret) in [
            // Short PINs, passcodes and numeric passwords: the 4..=7 range is live.
            ("PIN=4821", "4821"),
            ("pin = 0042", "0042"),
            ("PASSCODE: \"123456\"", "123456"),
            ("\"password\": \"1234567\"", "1234567"),
            ("pin: '9876543'", "9876543"),
            // A random-letter token in backticks after `token:` / `secret:`.
            ("CLIENT_SECRET=`kqzwxvtrplmnbcdf`", "kqzwxvtrplmnbcdf"),
            ("client secret: `kqzwxvtrplmnbcdf`", "kqzwxvtrplmnbcdf"),
            ("token: `qwertyuiopasdfgh`", "qwertyuiopasdfgh"),
            // A camelCase / PascalCase password.
            (
                "\"password\": \"CorrectHorseBatteryStaple\"",
                "CorrectHorseBatteryStaple",
            ),
            ("secret = 'SummerWinterSpring'", "SummerWinterSpring"),
            // A path-like password or token.
            (
                "token: \"live/0123456789abcdef0123456789abcdef\"",
                "live/0123456789abcdef0123456789abcdef",
            ),
            (
                "password = \"summer2026/winter2026\"",
                "summer2026/winter2026",
            ),
            // The name repeated is a real weak credential.
            ("admin_password = \"adminpassword\"", "adminpassword"),
            ("db_password: 'dbpassword'", "dbpassword"),
            ("API_TOKEN=\"api-token\"", "api-token"),
            // Still masked: mixed-case random, base64 with a `/`, an 8-digit PIN.
            ("token: \"aB3dE5fG7hJ9kL1m\"", "aB3dE5fG7hJ9kL1m"),
            ("token: \"xKjQpLmZrTbVnWcY\"", "xKjQpLmZrTbVnWcY"),
            (
                "secret = 'wJalrXUtnFEMI/K7MDENG/bPxRfiCYsD9fT3kQ'",
                "wJalrXUtnFEMI",
            ),
            ("PIN=12345678", "12345678"),
            ("password: \"correcthorsebattery\"", "correcthorsebattery"),
            ("api_key = 'channeltoken-7f3a'", "channeltoken-7f3a"),
        ] {
            let out = redact_agent_output(text);
            assert!(out.contains("REDACTED"), "{text} -> {out}");
            assert!(!out.contains(secret), "{text} -> {out}");
        }
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

    /// KT-1528 — the name shapes are DERIVED from the vendor part of `RAW_PATTERNS`
    /// (same body, same minimum lengths, only the `\b` anchors dropped): one shape per
    /// vendor pattern, and a sample of each vendor is found inside a longer name,
    /// before and after a label.
    #[test]
    fn name_token_shapes_cover_every_vendor_pattern() {
        assert_eq!(
            RAW_PATTERNS.len() - CATCH_ALL_PATTERNS,
            name_token_shapes().count(),
            "a vendor pattern of RAW_PATTERNS has no shape for key names"
        );
        assert_eq!(NAME_TOKEN_REGEXES.len(), name_token_shapes().count());
        // Mechanical: no anchor is left, and every body is the vendor's own.
        for (shape, (src, _, _)) in
            name_token_shapes().zip(RAW_PATTERNS.iter().skip(CATCH_ALL_PATTERNS))
        {
            assert!(
                !shape.starts_with(r"\b") && !shape.ends_with(r"\b"),
                "{shape}"
            );
            assert!(src.contains(shape), "{shape} is not a body of {src}");
        }
        let samples = [
            "sk-clear-abcdefghijklmnopqrstuvwxyz0123456789",
            "p8e-abcdefghijklmnop",
            "AIzaSyA1234567890abcdefghijklmnopqrstuvw",
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
            "github_pat_11ABCDEFG0abcdefghijklmnopqr_abcdefghijklmnopqrstuvwxyz0123456789",
            "ya29.a0AfH6SMBabcdefghijklmnopqrstuvwxyz",
            "1//0abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOP",
            "xoxb-123456789012-123456789012",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghij",
            "AKIAIOSFODNN7EXAMPLE",
            "ASIAIOSFODNN7EXAMPLE",
            "rk_live_abcdefghij1234567890",
            "sk_live_abcdefghij1234567890",
            "glpat-abcdefghijklmnopqrstuvwxyz",
            "npm_abcdefghijklmnopqrstuvwxyz0123456789",
            "hf_abcdefghijklmnopqrstuvwxyz0123456789",
            "kbt_abcdefghijklmnopqrstuvwx",
            "nvapi-abcdefghijklmnopqrstuvwxyz0123456789",
        ];
        assert_eq!(samples.len(), name_token_shapes().count());
        for sample in samples {
            assert!(looks_like_secret(sample), "{sample}");
            for name in [
                sample.to_string(),
                format!("old_{sample}"),
                format!("old{sample}"),
                format!("{sample}_old"),
                format!("my_{sample}_old"),
            ] {
                assert!(name_embeds_vendor_token(&name), "{name}");
            }
        }
        for plain in [
            "port",
            "max_tokens",
            "model_id",
            "skip_tls",
            "sk_rate",
            "task_basic_info",
        ] {
            assert!(!name_embeds_vendor_token(plain), "{plain}");
        }
    }

    /// KT-1528 round 17 — a name holds a vendor token at the VENDOR's own minimum
    /// length, label glued or not (the name shapes used to be stricter than
    /// `RAW_PATTERNS`: `p8e-` 10 against 8, `glpat-` 20 against 16).
    #[test]
    fn a_name_holds_a_vendor_token_at_the_vendors_own_minimum() {
        for name in [
            "old_p8e-12345678",
            "oldp8e-12345678",
            "old_glpat-abcdefghijklmnop",
            "myglpat-abcdefghijklmnop",
            "glpat-abcdefghijklmnop_old",
            "kbt_abcdefghijklmnop_old",
            "sk_live_abcdefghijklmnop_old",
            "old_ghp_abcdefghijklmnopqrstuvwxyz0123_x",
            "hf_abcdefghijklmnopqrstuvwxyz0123_x",
        ] {
            assert!(name_embeds_vendor_token(name), "{name}");
        }
        // One character under the minimum: not a token (the vendor pattern says so).
        for name in [
            "old_p8e-1234567",
            "old_glpat-abcdefghijklmno",
            "old_kbt_abcdefghijklmno",
            "old_sk_live_abcdefghijklmno",
        ] {
            assert!(!name_embeds_vendor_token(name), "{name}");
        }
    }

    /// KT-1528 rounds 19-20 — `sk-` inside an ordinary word (`di|sk-`, `ta|sk-`)
    /// is not a token unless the match holds a digit or a capital; every other
    /// vendor shape, and `sk-` after a separator or at the start, is found glued
    /// or not, and a rejected glued match never hides a later one.
    #[test]
    fn a_vendor_prefix_inside_a_word_is_not_a_token() {
        for name in [
            "disk-cache-max-size-bytes",
            "task-runner-max-concurrency",
            "mask-sensitive-headers-in-logs",
            "risk-assessment-threshold-ms",
            "kiosk-mode-refresh-interval-seconds",
            "desk-booking-calendar-sync",
            "ask-before-running-shell-commands",
        ] {
            assert!(!name_embeds_vendor_token(name), "{name}");
        }
        for name in [
            "sk-550e8400-e29b-41d4-a716-446655440000",
            "old_sk-550e8400-e29b-41d4-a716-446655440000",
            "1sk-abcdefghijklmnopqrst1",
            "my_AKIAIOSFODNN7EXAMPLE",
            "myAKIAIOSFODNN7EXAMPLE",
            "botxoxb-123456789012-123456789012",
            "disk-cache-max-size-bytes_sk-abcdefghijklmnopqrstuvwxyz",
            "oldsk-abcdefghij0123456789",
            "oldsk-abcdefghijKlmnopqrstuv",
            // Round 20: every other vendor shape counts glued to a label even
            // without a digit (a real token may hold none).
            "myAKIAIOSFODNNXEXAMPLE",
            "oldASIAIOSFODNNXEXAMPLE",
            "myhf_aBcDeFgHiJkLmNoPqRsTuVwXyZaBcDeFgH",
            "myglpat-abcdefghijklmnop",
            "myghp_abcdefghijklmnopqrstuvwxyzABCDEF",
            "mysk_live_abcdefghijklmnopqrst",
            "botxoxb-abcdefghijklmnop",
            "mynpm_abcdefghijklmnopqrstuvwxyzABCD",
            "mykbt_abcdefghijklmnop",
            "oldAKIAIOSFODNNXEXAMPLE_550e8400-e29b-41d4-a716-446655440000",
        ] {
            assert!(name_embeds_vendor_token(name), "{name}");
        }
    }

    // ── Gates: the fast path may never change what gets redacted ───────

    /// `apply_patterns` as it was before gates: every regex, every time.
    fn apply_patterns_ungated(input: &str, patterns: &[Pattern]) -> (String, usize) {
        let mut out = input.to_string();
        let mut count = 0usize;
        for pat in patterns {
            // A match the pattern says to leave is no gate: it is left here too.
            let left = |captures: &regex_lite::Captures<'_>| {
                pat.leave.is_some_and(|leave| leave(captures, &out))
            };
            let changed = pat
                .re
                .captures_iter(&out)
                .filter(|captures| {
                    let Some(matched) = captures.get(0) else {
                        return false;
                    };
                    let mut replacement = String::new();
                    captures.expand(pat.replacement, &mut replacement);
                    replacement != matched.as_str() && !left(captures)
                })
                .count();
            if changed == 0 {
                continue;
            }
            out = pat
                .re
                .replace_all(&out, |captures: &regex_lite::Captures<'_>| {
                    let mut replacement = String::new();
                    if left(captures) {
                        replacement.push_str(&captures[0]);
                    } else {
                        captures.expand(pat.replacement, &mut replacement);
                    }
                    replacement
                })
                .to_string();
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
        assert_eq!(ASSIGNMENT.len(), 22);
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
            r#"x-api-token: \"Ab3x Z9\" done"#,
            r#"token: \"Ab3xZ9"#,
            r#"Authorization: Bearer \"Ab3xZ9Qw7Lm2Ns5Pt8Rv\""#,
            r#"{"password":"a\"b"}"#,
            // KT-2039
            r#"api_key: Some("Fake1Some2Value3Abc")"#,
            "password: Some(\n    \"Fake4Pretty5Value6\",\n)",
            r#"token = Secret::new(\"Fake1Some2\")"#,
            "{ 'x-api-key': 'Fake7Quoted8Key9' }",
            "nvapi-Fake0Nvidia1Key2Value3Abcdef4",
            "rediss://default:Fake5Redis6Pass@cache.internal",
            "redis://:Fake5Redis6Pass@cache.internal",
            // KT-2044
            r#"api_key: Some(Token("Fake1Nest2Value3"))"#,
            "password: Some(\n    Token(\n        \"Fake1Nest2Value3\",",
            r#"password: Some("Fake2Cut3Value4"#,
            "token = Secret::new('fake-cut-hunter2",
            r#"token: Some(Token(\"Fake2Cut3Value4"#,
            r#"{ 'x-api-key': "it's-Fake5Mixed6Key7" }"#,
            r#"{ "client_secret": 'Fake5Mixed6Key7' }"#,
            // KT-2053
            r#"left: "Cfg { token: Some(\"Fake1\\\"Nest2Value3\") }""#,
            r#"token: \"Fake1\\\"Nest2Value3"#,
            r#"password = strdup ("Fake3Gnu4Hunter5");"#,
            "token: Secret::new\t('Fake3Gnu4Hunter5')",
            r#"left: "Cfg {\n    token: Some(\n        \"Fake1Nest2Value3\",\n    ),\n}""#,
            r#"token: Some(\n        \"Fake1Nest2Value3"#,
            r#"password: Secret::<String>::new("Fake6Turbo7Fish8")"#,
            r#"token = parse::<Secret<String>>('Fake6Turbo7Fish8"#,
            r#"token: unwrap_or_else(|| "Fake6Turbo7Fish8")"#,
            r#"token = || "Fake6Turbo7Fish8""#,
            "api_key = move |_| 'Fake6Turbo7Fish8'",
            r#"{ \"token\": 'Fake5Mixed6Key7' }"#,
            r#"{ 'token': \"Fake5\\\"Mixed6Key7\" }"#,
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
        // ASSIGNMENT is ordered: double quote, single quote, backtick, escaped, the
        // three wrapped in a call (KT-2039), the four quoted ones cut before their
        // closing quote, the three wrapped ones cut the same way (KT-2044), bare,
        // then the seven quoted-key shapes (KT-1656, KT-2039, KT-2044, KT-2053).
        let admitted = |text: &str| -> Vec<bool> {
            ASSIGNMENT.iter().map(|pat| pat.gate.admits(text)).collect()
        };
        let (t, f) = (true, false);
        // The quote sits before the name, not after the `:`: only the bare
        // value pattern has anything to do.
        assert_eq!(
            admitted("curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com"),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted("password: \"hunter2\""),
            [t, f, f, f, f, f, f, t, f, f, f, f, f, f, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted("token =\n  'hunter2'"),
            [f, t, f, f, f, f, f, f, t, f, f, f, f, f, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted("secret=`hunter2`"),
            [f, f, t, f, f, f, f, f, f, t, f, f, f, f, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted(r#"token: \"hunter2\""#),
            [f, f, f, t, f, f, f, f, f, f, t, f, f, f, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted(r#"api_key: Some("hunter2")"#),
            [f, f, f, f, t, t, t, f, f, f, f, t, t, t, t, f, f, f, f, f, f, f]
        );
        assert_eq!(
            admitted("{ 'x-api-key': 'hunter2' }"),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, t, f, f, f, f]
        );
        assert_eq!(
            admitted(r#"{ 'x-api-key': "it's-hunter2" }"#),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, t, f, f, f]
        );
        assert_eq!(
            admitted(r#"{ "token": 'hunter2' }"#),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, t, t, f]
        );
        // KT-2053 — the same keys inside a string printed with `{:?}`.
        assert_eq!(
            admitted(r#"{ \"token\": 'hunter2' }"#),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, t, t, f]
        );
        assert_eq!(
            admitted(r#"{ 'token': \"hunter2\" }"#),
            [f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, f, t]
        );
        // No assignment at all: nothing runs, whatever quotes the text holds.
        assert_eq!(admitted("say \"hello\" to `them`, it's fine"), [false; 22]);
        // Each pattern that is turned away really has no match to make.
        for text in [
            "curl -H 'x-api-key: ${SERVICE_KEY}' https://example.com",
            "password: \"hunter2\"",
            "token =\n  'hunter2'",
            "secret=`hunter2`",
            r#"token: \"hunter2\""#,
            r#"api_key: Some("hunter2")"#,
            "{ 'x-api-key': 'hunter2' }",
            r#"{ 'x-api-key': "it's-hunter2" }"#,
            r#"{ "token": 'hunter2' }"#,
            r#"{ \"token\": 'hunter2' }"#,
            r#"{ 'token': \"hunter2\" }"#,
        ] {
            for pat in ASSIGNMENT.iter().filter(|pat| !pat.gate.admits(text)) {
                assert!(!pat.re.is_match(text), "{} on {text:?}", pat.re.as_str());
            }
        }
    }

    // ── KT-2039: the shapes a red test run prints ─────────────────────
    //
    // Every value below is fake.

    /// Asserts `secret` is gone from `redact_stored_text(raw)`, that the output
    /// keeps `kept`, and that a second pass changes nothing.
    fn assert_stored_masks(raw: &str, secret: &str, kept: &str) {
        let out = redact_stored_text(raw);
        assert!(!out.contains(secret), "{secret} leaked: {raw:?} -> {out:?}");
        assert!(out.contains("***REDACTED***"), "{raw:?} -> {out:?}");
        assert!(out.contains(kept), "{kept:?} lost: {raw:?} -> {out:?}");
        assert_eq!(redact_stored_text(&out), out, "idempotent on {raw:?}");
    }

    #[test]
    fn a_secret_wrapped_in_some_is_masked() {
        for (raw, secret, kept) in [
            (
                r#"api_key: Some("Fake1Some2Value3Abc")"#,
                "Fake1Some2Value3Abc",
                "api_key: Some(",
            ),
            (
                r#"ExternalHttpRuntime { model: "m", api_key: Some("Fake1Some2Value3Abc"), base: None }"#,
                "Fake1Some2Value3Abc",
                r#"model: "m""#,
            ),
            (
                "password = Secret::new('fake-hunter2')",
                "fake-hunter2",
                "Secret::new(",
            ),
            (
                "token: Some( \"Fake1Some2Value3Abc\" )",
                "Fake1Some2Value3Abc",
                "token: Some(",
            ),
            // Inside a `{:?}`-printed string, where the quotes are escaped.
            (
                r#"left: "Cfg { token: Some(\"Fake1Some2Value3Abc\") }""#,
                "Fake1Some2Value3Abc",
                "left: ",
            ),
        ] {
            assert_stored_masks(raw, secret, kept);
        }
    }

    #[test]
    fn a_secret_wrapped_in_some_is_masked_across_lines_as_pretty_debug_prints_it() {
        let raw = "Runtime {\n    model: \"m\",\n    password: Some(\n        \"Fake4Pretty5Value6\",\n    ),\n    retries: 3,\n}";
        assert_stored_masks(raw, "Fake4Pretty5Value6", "retries: 3");
        let crlf = raw.replace('\n', "\r\n");
        assert_stored_masks(&crlf, "Fake4Pretty5Value6", "retries: 3");
    }

    #[test]
    fn a_stored_agent_output_masks_a_wrapped_credential_too() {
        let out = redact_agent_output(r#"api_key: Some("Fake1Some2Value3Abc")"#);
        assert!(!out.contains("Fake1Some2Value3Abc"), "{out}");
        // …and keeps a wrapped placeholder.
        let placeholder = r#"api_key: Some("<your-api-key>")"#;
        assert_eq!(redact_agent_output(placeholder), placeholder);
    }

    #[test]
    fn a_single_quoted_key_is_masked() {
        for (raw, secret, kept) in [
            (
                "{ 'x-api-key': 'Fake7Quoted8Key9', accept: 'application/json' }",
                "Fake7Quoted8Key9",
                "accept: 'application/json'",
            ),
            (
                "headers: {\n  'Authorization-Token': 'Fake7Quoted8Key9',\n  'content-type': 'text/plain'\n}",
                "Fake7Quoted8Key9",
                "'content-type': 'text/plain'",
            ),
            (
                "{'password': 'fake-hunter2'}",
                "fake-hunter2",
                "'password': '",
            ),
        ] {
            assert_stored_masks(raw, secret, kept);
        }
    }

    // ── KT-2044: the shapes the second review found ───────────────────
    //
    // Every value below is fake.

    #[test]
    fn a_secret_wrapped_in_nested_calls_is_masked_and_the_calls_kept() {
        for (raw, expected) in [
            (
                r#"api_key: Some(Token("Fake1Nest2Value3"))"#,
                r#"api_key: Some(Token("***REDACTED***"))"#,
            ),
            (
                "password = Secret::new(Box::new('fake-nested-hunter2'))",
                "password = Secret::new(Box::new('***REDACTED***'))",
            ),
            (
                "token: Some( Token( \"Fake1Nest2Value3\" ) )",
                "token: Some( Token( \"***REDACTED***\" ) )",
            ),
            (
                "Config {\n    password: Some(\n        Token(\n            \"Fake1Nest2Value3\",\n        ),\n    ),\n    retries: 3,\n}",
                "Config {\n    password: Some(\n        Token(\n            \"***REDACTED***\",\n        ),\n    ),\n    retries: 3,\n}",
            ),
            // Inside a `{:?}`-printed string, where the quotes are escaped.
            (
                r#"left: "Cfg { token: Some(Token(\"Fake1Nest2Value3\")) }""#,
                r#"left: "Cfg { token: Some(Token(\"***REDACTED***\")) }""#,
            ),
        ] {
            assert_eq!(redact_stored_text(raw), expected, "{raw:?}");
            assert_eq!(redact_stored_text(expected), expected, "idempotent");
        }
        let out = redact_agent_output(r#"api_key: Some(Token("Fake1Nest2Value3"))"#);
        assert!(!out.contains("Fake1Nest2Value3"), "{out}");
        // …and keeps a placeholder wrapped twice.
        let placeholder = r#"api_key: Some(Token("<your-api-key>"))"#;
        assert_eq!(redact_agent_output(placeholder), placeholder);
    }

    #[test]
    fn a_wrapped_secret_cut_before_its_closing_quote_is_masked_and_the_call_kept() {
        for (raw, expected) in [
            (
                r#"password: Some("Fake2Cut3Value4"#,
                r#"password: Some("***REDACTED***"#,
            ),
            (
                r#"api_key: Some(Token("Fake2Cut3Value4"#,
                r#"api_key: Some(Token("***REDACTED***"#,
            ),
            (
                "token = Secret::new('fake-cut-hunter2",
                "token = Secret::new('***REDACTED***",
            ),
            (
                "Config {\n    password: Some(\n        \"Fake2Cut3Value4",
                "Config {\n    password: Some(\n        \"***REDACTED***",
            ),
            (
                r#"left: "Cfg { token: Some(\"Fake2Cut3Value4"#,
                r#"left: "Cfg { token: Some(\"***REDACTED***"#,
            ),
            // A lone backslash ending the text belongs to the value.
            (
                r#"password: Some("Fake2Cut3Value4\"#,
                r#"password: Some("***REDACTED***"#,
            ),
        ] {
            assert_eq!(redact_stored_text(raw), expected, "{raw:?}");
            assert_eq!(redact_stored_text(expected), expected, "idempotent");
        }
    }

    #[test]
    fn a_quoted_key_with_the_other_quote_on_its_value_is_masked() {
        for (raw, expected) in [
            // Node's `util.inspect` and Python's `repr` switch to double quotes
            // for a string that holds a single one.
            (
                r#"{ 'x-api-key': "it's-Fake5Mixed6Key7", accept: 'application/json' }"#,
                r#"{ 'x-api-key': "***REDACTED***", accept: 'application/json' }"#,
            ),
            (
                r#"{'password': "fake-it's-hunter2"}"#,
                r#"{'password': "***REDACTED***"}"#,
            ),
            (
                r#"{ "client_secret": 'Fake5Mixed6Key7' }"#,
                r#"{ "client_secret": '***REDACTED***' }"#,
            ),
        ] {
            assert_eq!(redact_stored_text(raw), expected, "{raw:?}");
            assert_eq!(redact_stored_text(expected), expected, "idempotent");
        }
    }

    // ── KT-2053: the shapes the third review found ────────────────────
    //
    // Every value below is fake.

    /// Asserts that no byte of `secret` survives `raw` through both
    /// `redact_stored_text` and `redact_agent_output`, that the stored output is
    /// `expected` and that a second pass changes neither.
    fn assert_both_mask(raw: &str, secret: &str, expected: &str) {
        let stored = redact_stored_text(raw);
        assert_eq!(stored, expected, "stored: {raw:?}");
        assert_eq!(redact_stored_text(&stored), stored, "stored idempotent");
        let agent = redact_agent_output(raw);
        for part in [
            secret,
            &secret[secret.len() / 2..],
            &secret[..secret.len() / 2],
        ] {
            assert!(
                !agent.contains(part),
                "agent: {part} leaked: {raw:?} -> {agent:?}"
            );
        }
        assert!(
            agent.contains("***REDACTED***"),
            "agent: {raw:?} -> {agent:?}"
        );
        assert_eq!(redact_agent_output(&agent), agent, "agent idempotent");
    }

    #[test]
    fn an_escaped_quote_inside_a_printed_value_does_not_end_it() {
        // A `"` inside the secret, in a string printed with `{:?}`: `\\\"`.
        for (raw, expected) in [
            (
                r#"left: "Cfg { token: Some(\"Fake1\\\"Nest2Value3\") }""#,
                r#"left: "Cfg { token: Some(\"***REDACTED***\") }""#,
            ),
            (
                r#"left: "Cfg { token: \"Fake1\\\"Nest2Value3\", retries: 3 }""#,
                r#"left: "Cfg { token: \"***REDACTED***\", retries: 3 }""#,
            ),
            (
                r#"left: "{\"token\": \"Fake1\\\"Nest2Value3\", \"id\": 7}""#,
                r#"left: "{\"token\": \"***REDACTED***\", \"id\": 7}""#,
            ),
            // Cut before its closing quote.
            (
                r#"left: "Cfg { token: Some(\"Fake1\\\"Nest2Value3"#,
                r#"left: "Cfg { token: Some(\"***REDACTED***"#,
            ),
            (
                r#"left: "Cfg { token: \"Fake1\\\"Nest2Value3"#,
                r#"left: "Cfg { token: \"***REDACTED***"#,
            ),
            // A backslash inside the secret (`\\\\`) is read whole too.
            (
                r#"left: "Cfg { token: \"Fake1\\\\Nest2Value3\" }""#,
                r#"left: "Cfg { token: \"***REDACTED***\" }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake1Nest2Value3", expected);
            assert!(!redact_stored_text(raw).contains("Nest2Value3"), "{raw:?}");
            assert!(!redact_agent_output(raw).contains("Nest2Value3"), "{raw:?}");
        }
    }

    #[test]
    fn a_call_with_a_space_before_its_parenthesis_keeps_the_call_and_masks_the_value() {
        for (raw, expected) in [
            (
                r#"password = strdup ("Fake3Gnu4Hunter5");"#,
                r#"password = strdup ("***REDACTED***");"#,
            ),
            (
                "token: Secret::new\t('Fake3Gnu4Hunter5')",
                "token: Secret::new\t('***REDACTED***')",
            ),
            (
                r#"api_key = wrap (Some ("Fake3Gnu4Hunter5"))"#,
                r#"api_key = wrap (Some ("***REDACTED***"))"#,
            ),
        ] {
            assert_both_mask(raw, "Fake3Gnu4Hunter5", expected);
        }
    }

    #[test]
    fn pretty_debug_printed_inside_a_string_is_masked_and_the_call_kept() {
        // `{:?}` of a `{:#?}` dump: the newlines are a literal `\n`.
        for (raw, expected) in [
            (
                r#"left: "Cfg {\n    token: Some(\n        \"Fake1Nest2Value3\",\n    ),\n}""#,
                r#"left: "Cfg {\n    token: Some(\n        \"***REDACTED***\",\n    ),\n}""#,
            ),
            (
                r#"left: "Cfg {\r\n    password: Some(\r\n        Token(\r\n            \"Fake1Nest2Value3\",\r\n"#,
                r#"left: "Cfg {\r\n    password: Some(\r\n        Token(\r\n            \"***REDACTED***\",\r\n"#,
            ),
            // Cut before its closing quote.
            (
                r#"left: "Cfg {\n    token: Some(\n        \"Fake1Nest2Value3"#,
                r#"left: "Cfg {\n    token: Some(\n        \"***REDACTED***"#,
            ),
        ] {
            assert_both_mask(raw, "Fake1Nest2Value3", expected);
        }
    }

    #[test]
    fn turbofish_and_closure_openers_keep_the_call_and_mask_the_value() {
        for (raw, expected) in [
            (
                r#"password: Secret::<String>::new("Fake6Turbo7Fish8")"#,
                r#"password: Secret::<String>::new("***REDACTED***")"#,
            ),
            (
                r#"token = Into::<Secret>::into("Fake6Turbo7Fish8")"#,
                r#"token = Into::<Secret>::into("***REDACTED***")"#,
            ),
            (
                r#"token = parse::<Secret<String>>("Fake6Turbo7Fish8")"#,
                r#"token = parse::<Secret<String>>("***REDACTED***")"#,
            ),
            (
                r#"token: unwrap_or_else(|| "Fake6Turbo7Fish8")"#,
                r#"token: unwrap_or_else(|| "***REDACTED***")"#,
            ),
            (
                "api_key = get_or_insert_with(move |_| 'Fake6Turbo7Fish8')",
                "api_key = get_or_insert_with(move |_| '***REDACTED***')",
            ),
            (
                r#"left: "Cfg { token: Secret::<String>::new(\"Fake6Turbo7Fish8\") }""#,
                r#"left: "Cfg { token: Secret::<String>::new(\"***REDACTED***\") }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake6Turbo7Fish8", expected);
        }
    }

    #[test]
    fn an_escaped_key_with_the_other_quote_on_its_value_is_masked() {
        for (raw, expected) in [
            (
                r#"left: "{ \"token\": 'Fake5Mixed6Key7', \"id\": 7 }""#,
                r#"left: "{ \"token\": '***REDACTED***', \"id\": 7 }""#,
            ),
            (
                r#"left: "{ 'token': \"Fake5Mixed6Key7\", 'id': 7 }""#,
                r#"left: "{ 'token': \"***REDACTED***\", 'id': 7 }""#,
            ),
            // A `"` inside the value, printed `\\\"`.
            (
                r#"left: "{ 'client_secret': \"Fake5\\\"Mixed6Key7\" }""#,
                r#"left: "{ 'client_secret': \"***REDACTED***\" }""#,
            ),
            (
                r#"left: "{ \"client_secret\": 'Fake5\\\"Mixed6Key7' }""#,
                r#"left: "{ \"client_secret\": '***REDACTED***' }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake5Mixed6Key7", expected);
            assert!(!redact_stored_text(raw).contains("Mixed6Key7"), "{raw:?}");
        }
    }

    #[test]
    fn the_name_before_a_call_reads_the_call_syntax_of_the_patterns() {
        for (prefix, name) in [
            ("password = strdup (", "password"),
            ("password = strdup\t(", "password"),
            ("api_key = wrap (Some (", "api_key"),
            ("password: Secret::<String>::new(", "password"),
            ("token = Into::<Secret>::into(", "token"),
            ("token = parse::<Secret<String>>(", "token"),
            ("password: Secret::<Vec<Box<str>>>::new(", "password"),
            ("token: Secret::<&'static str>::new(", "token"),
            ("token: Wrap::<HashMap<String, Vec<u8>>>::from(", "token"),
            ("token: unwrap_or_else(|| ", "token"),
            ("api_key = get_or_insert_with(move |_| '", "api_key"),
            (r#"token: Some(\n        \""#, "token"),
            (
                r#"password: Some(\r\n        Token(\r\n            \""#,
                "password",
            ),
            // Unchanged readings.
            ("api_key: Some(Token(", "api_key"),
            (r#"  "GEMINI_API_KEY": ""#, "GEMINI_API_KEY"),
            ("export GITHUB_TOKEN=", "GITHUB_TOKEN"),
        ] {
            assert_eq!(name_before_value(prefix), name, "{prefix:?}");
        }
    }

    #[test]
    fn the_call_openers_read_the_call_syntax_of_the_patterns() {
        for (text, rest, openers) in [
            ("strdup (\"x\"", "\"x\"", 1),
            ("strdup\t(  \"x\"", "\"x\"", 1),
            ("wrap (Some (\"x\"", "\"x\"", 2),
            ("Secret::<String>::new(\"x\"", "\"x\"", 1),
            ("Into::<Secret>::into(\"x\"", "\"x\"", 1),
            ("parse::<Secret<String>>(\"x\"", "\"x\"", 1),
            ("Secret::<Vec<Box<str>>>::new(\"x\"", "\"x\"", 1),
            ("Secret::<&'static str>::new(\"x\"", "\"x\"", 1),
            ("unwrap_or_else(|| \"x\"", "\"x\"", 2),
            ("get_or_insert_with(move |_| 'x'", "'x'", 2),
            ("|| \"x\"", "\"x\"", 1),
            (r#"Some(\n        Token(\n    \"x"#, r#"\"x"#, 2),
            (r#"Some(\r\n\t\"x"#, r#"\"x"#, 1),
            // Not openers.
            ("strdup \"x\"", "strdup \"x\"", 0),
            ("Secret::<String \"x\"", "Secret::<String \"x\"", 0),
            ("|x \"x\"", "|x \"x\"", 0),
            ("move \"x\"", "move \"x\"", 0),
        ] {
            assert_eq!(past_call_openers(text), (rest, openers), "{text:?}");
        }
        for (text, length) in [
            ("strdup (", 6),
            ("Secret::<String>::new(", 21),
            ("Into::<Secret>::into(", 20),
            ("parse::<Secret<String>>(", 23),
            ("Secret::<Vec<Box<str>>>::new(", 28),
            ("Secret::<&'static str>::new(", 27),
            ("Secret::<String", 6),
            ("Some(", 4),
            ("(", 0),
        ] {
            assert_eq!(call_path_len(text), length, "{text:?}");
        }
    }

    #[test]
    fn a_kt_2035_cargo_failure_keeps_its_result_line_with_the_new_shapes() {
        let block = concat!(
            "---- core::cfg::tests::loads stdout ----\n",
            "thread 'core::cfg::tests::loads' panicked at src/core/cfg.rs:10:5:\n",
            "assertion `left == right` failed\n",
            r#"  left: "Cfg { token: Some(\"Fake1\\\"Nest2Value3\"), retries: 3 }""#,
            "\n",
            r#" right: "Cfg {\n    password: Some(\n        \"Fake4Pretty5Value6\",\n    ),\n}""#,
            "\n",
            "password = strdup (\"Fake3Gnu4Hunter5\");\n",
            "\n",
            "test result: FAILED. 41 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
        );
        for out in [redact_stored_text(block), redact_agent_output(block)] {
            for secret in ["Nest2Value3", "Fake4Pretty5Value6", "Fake3Gnu4Hunter5"] {
                assert!(!out.contains(secret), "{secret} leaked: {out}");
            }
            assert!(
                out.contains(
                    "test result: FAILED. 41 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out"
                ),
                "{out}"
            );
            assert!(out.contains("retries: 3"), "{out}");
        }
    }

    // ── KT-2053 review: every literal of a secret's value expression ──
    //
    // Every value below is fake.

    #[test]
    fn a_typed_declaration_masks_the_literal_after_its_type() {
        for (raw, expected) in [
            (
                r#"let token: &str = "Fake9Typed0Decl1";"#,
                r#"let token: ***REDACTED*** = "***REDACTED***";"#,
            ),
            (
                r#"const apiKey: string = "Fake9Typed0Decl1";"#,
                r#"const apiKey: ***REDACTED*** = "***REDACTED***";"#,
            ),
            (
                r#"password: str = "Fake9Typed0Decl1""#,
                r#"password: ***REDACTED*** = "***REDACTED***""#,
            ),
            (
                r#"api_key: Option<String> = Some("Fake9Typed0Decl1".into());"#,
                r#"api_key: ***REDACTED*** = Some("***REDACTED***".into());"#,
            ),
            (
                r#"left: "let token: &str = \"Fake9Typed0Decl1\";""#,
                r#"left: "let token: ***REDACTED*** = \"***REDACTED***\";""#,
            ),
        ] {
            assert_both_mask(raw, "Fake9Typed0Decl1", expected);
        }
    }

    #[test]
    fn a_prefixed_literal_is_masked() {
        for (raw, expected) in [
            (
                r#"password = &"Fake2Prefix3Lit4""#,
                r#"password = ***REDACTED***"***REDACTED***""#,
            ),
            (
                r#"password = b"Fake2Prefix3Lit4""#,
                r#"password = ***REDACTED***"***REDACTED***""#,
            ),
            (
                r#"token = r"Fake2Prefix3Lit4""#,
                r#"token = ***REDACTED***"***REDACTED***""#,
            ),
            (
                r##"token = r#"Fake2"Prefix3Lit4"#;"##,
                r##"token = ***REDACTED***"***REDACTED***"#;"##,
            ),
            (
                r#"secret = f"Fake2Prefix3Lit4""#,
                r#"secret = ***REDACTED***"***REDACTED***""#,
            ),
            (
                "secret = u'Fake2Prefix3Lit4'",
                "secret = ***REDACTED***'***REDACTED***'",
            ),
            (
                r#"left: "Cfg { token: b\"Fake2Prefix3Lit4\", retries: 3 }""#,
                // The bare pattern takes the prefix with the opening quote.
                r#"left: "Cfg { token: ***REDACTED***\", retries: 3 }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake2Prefix3Lit4", expected);
        }
    }

    #[test]
    fn a_dotted_or_method_call_masks_its_literals() {
        for (raw, expected) in [
            (
                r#"password = os.getenv("DB_PASSWORD", "Fake5Dotted6Call7")"#,
                r#"password = ***REDACTED***"***REDACTED***", "***REDACTED***")"#,
            ),
            (
                r#"token = self.secret("Fake5Dotted6Call7")"#,
                r#"token = ***REDACTED***"***REDACTED***")"#,
            ),
            (
                "api_key = config.get('Fake5Dotted6Call7')",
                "api_key = ***REDACTED***'***REDACTED***')",
            ),
            // A default buried in a method chain is in the expression too.
            (
                r#"token = env::var("TOKEN").unwrap_or("Fake5Dotted6Call7");"#,
                r#"token = env::var("***REDACTED***").unwrap_or("***REDACTED***");"#,
            ),
        ] {
            assert_both_mask(raw, "Fake5Dotted6Call7", expected);
        }
    }

    #[test]
    fn a_later_literal_of_a_recognised_call_is_masked() {
        for (raw, expected) in [
            (
                r#"api_key = env::var_or("KEY", "Fake8Second9Arg0")"#,
                r#"api_key = env::var_or("***REDACTED***", "***REDACTED***")"#,
            ),
            (
                r#"token: Some(Pair('id', "Fake8Second9Arg0")), retries: 3"#,
                r#"token: Some(Pair('***REDACTED***', "***REDACTED***")), retries: 3"#,
            ),
            (
                r#"left: "Cfg { token: pick(\"KEY\", \"Fake8Second9Arg0\"), retries: 3 }""#,
                r#"left: "Cfg { token: pick(\"***REDACTED***\", \"***REDACTED***\"), retries: 3 }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake8Second9Arg0", expected);
        }
    }

    #[test]
    fn deep_generics_and_lifetimes_keep_the_call_and_mask_the_value() {
        for (raw, expected) in [
            (
                r#"password: Secret::<Vec<Box<str>>>::new("Fake1Deep2Generic3")"#,
                r#"password: Secret::<Vec<Box<str>>>::new("***REDACTED***")"#,
            ),
            (
                r#"token: Secret::<&'static str>::new("Fake1Deep2Generic3")"#,
                r#"token: Secret::<&'static str>::new("***REDACTED***")"#,
            ),
            (
                r#"token = Wrap::<HashMap<String, Vec<u8>>>::from('Fake1Deep2Generic3');"#,
                r#"token = Wrap::<HashMap<String, Vec<u8>>>::from('***REDACTED***');"#,
            ),
            (
                r#"left: "Cfg { token: Secret::<&'static str>::new(\"Fake1Deep2Generic3\") }""#,
                r#"left: "Cfg { token: Secret::<&'static str>::new(\"***REDACTED***\") }""#,
            ),
        ] {
            assert_both_mask(raw, "Fake1Deep2Generic3", expected);
        }
    }

    #[test]
    fn a_value_expression_ends_at_its_own_end() {
        for (raw, expected) in [
            (
                r#"Cfg { token: Some("Fake4End5Here6"), retries: 3, name: "kept" }"#,
                r#"Cfg { token: Some("***REDACTED***"), retries: 3, name: "kept" }"#,
            ),
            (
                "password = os.getenv(\"X\", 'Fake4End5Here6');\nlabel = \"kept\"",
                "password = ***REDACTED***\"***REDACTED***\", '***REDACTED***');\nlabel = \"kept\"",
            ),
            (
                r#"f(token: &"Fake4End5Here6") + g("kept")"#,
                r#"f(token: ***REDACTED***"***REDACTED***") + g("kept")"#,
            ),
            (
                r#"left: "Cfg { token: b\"Fake4End5Here6\" }", right: "kept""#,
                r#"left: "Cfg { token: ***REDACTED***\" }", right: "kept""#,
            ),
        ] {
            assert_both_mask(raw, "Fake4End5Here6", expected);
        }
    }

    #[test]
    fn text_with_no_secret_value_expression_is_left_alone() {
        for plain in [
            "max_tokens: 100",
            r#"token_count = f("abc")"#,
            r#"She said "hello" and left."#,
            r#"tokens = tokenize("some input")"#,
            "the user's token is valid, see 'docs'",
        ] {
            assert_eq!(redact_stored_text(plain), plain, "{plain:?}");
            assert_eq!(redact_agent_output(plain), plain, "{plain:?}");
        }
    }

    // ── KT-2053 review, round 2 ────────────────────────────────────────
    //
    // Every value below is fake.

    #[test]
    fn a_quote_after_a_dot_bang_or_question_mark_opens_the_value() {
        for (raw, expected) in [
            (
                r#"$password = $salt."Fake1Dot2Value3";"#,
                r#"$password = ***REDACTED***"***REDACTED***";"#,
            ),
            (
                "$token = 'sk_'.'Fake1Dot2Value3';",
                "$token = '***REDACTED***'.'***REDACTED***';",
            ),
            (
                r#"token = ok?"Fake1Dot2Value3":"x";"#,
                r#"token = ***REDACTED***"***REDACTED***":"***REDACTED***";"#,
            ),
            (
                r#"const token = env.TOKEN??"Fake1Dot2Value3";"#,
                r#"const token = ***REDACTED***"***REDACTED***";"#,
            ),
            (
                r#"token = !!"Fake1Dot2Value3""#,
                r#"token = ***REDACTED***"***REDACTED***""#,
            ),
            (
                r#"left: "$password = $salt.\"Fake1Dot2Value3\";""#,
                r#"left: "$password = ***REDACTED***\";""#,
            ),
        ] {
            assert_both_mask(raw, "Fake1Dot2Value3", expected);
        }
        // The quote that closes a sentence still ends the expression: what
        // follows it is kept (the `api::projects::resources` diff line).
        let prose = r#"{"a": "use password=X and the key Y.", "b": "c"}"#;
        assert_eq!(
            redact_stored_text(prose),
            r#"{"a": "use password=***REDACTED*** and the key Y.", "b": "c"}"#
        );
        assert_eq!(redact_agent_output(prose), prose);
    }

    #[test]
    fn a_value_continued_on_the_next_line_is_masked() {
        for (raw, expected) in [
            (
                "let token = env::var(\"TOKEN\")\n    .unwrap_or(\"Fake4Next5Line6\".into());",
                "let token = env::var(\"***REDACTED***\")\n    .unwrap_or(\"***REDACTED***\".into());",
            ),
            (
                "const token = process.env.TOKEN\n  || \"Fake4Next5Line6\";",
                "const token = ***REDACTED***\n  || \"***REDACTED***\";",
            ),
            (
                "const token = ok\n  ? \"Fake4Next5Line6\"\n  : \"\";\nlabel = \"kept\"",
                "const token = ***REDACTED***\n  ? \"***REDACTED***\"\n  : \"\";\nlabel = \"kept\"",
            ),
            (
                "+    let token = env::var(\"TOKEN\")\n+        .unwrap_or(\"Fake4Next5Line6\");",
                "+    let token = env::var(\"***REDACTED***\")\n+        .unwrap_or(\"***REDACTED***\");",
            ),
        ] {
            assert_both_mask(raw, "Fake4Next5Line6", expected);
        }
        // A line that starts anything else ends the expression: prose, the
        // next diff line, a `{:#?}` field.
        for (raw, kept) in [
            (
                "token: abc\nAnd then she said \"hello\" twice.",
                "\"hello\"",
            ),
            ("+token = \"Fake4Next5Line6\"\n+name = \"kept\"", "\"kept\""),
            (
                "Cfg {\n    token: Some(\n        \"Fake4Next5Line6\",\n    ),\n    name: \"kept\",\n}",
                "\"kept\"",
            ),
        ] {
            let stored = redact_stored_text(raw);
            assert!(!stored.contains("Fake4Next5Line6"), "{stored}");
            assert!(stored.contains(kept), "{raw:?} -> {stored:?}");
            assert!(redact_agent_output(raw).contains(kept), "{raw:?}");
        }
    }

    #[test]
    fn a_bare_quote_after_an_escaped_literal_closes_the_printed_string() {
        for (raw, expected) in [
            (
                "assertion `left == right` failed\n  left: \"token: \\\"Fake7Bare8Quote9\\\"\"\n right: \"kept\"\nnote: run with `RUST_BACKTRACE=1`",
                "assertion `left == right` failed\n  left: \"token: \\\"***REDACTED***\\\"\"\n right: \"kept\"\nnote: run with `RUST_BACKTRACE=1`",
            ),
            (
                "Cfg { note: \"token: \\\"Fake7Bare8Quote9\\\"\" }\nnext line kept",
                "Cfg { note: \"token: \\\"***REDACTED***\\\"\" }\nnext line kept",
            ),
            (
                "left: \"token: Some(\\\"Fake7Bare8Quote9\\\") \" right: \"kept\"",
                "left: \"token: Some(\\\"***REDACTED***\\\") \" right: \"kept\"",
            ),
        ] {
            assert_both_mask(raw, "Fake7Bare8Quote9", expected);
        }
    }

    #[test]
    fn a_value_on_the_next_line_or_after_a_trailing_operator_is_masked() {
        // Stored text: the bare pattern of `ASSIGNMENT` also masks the code
        // before the literal, as for the other new shapes (fail-closed).
        for (raw, expected) in [
            // Nothing read yet since the `=`: the value is on the next line.
            (
                "const token =\n  process.env.TOKEN ?? \"Fake3Trail4Op5\";",
                "const token =\n  ***REDACTED*** ?? \"***REDACTED***\";",
            ),
            (
                "password =\n    os.getenv(\"DB_PASSWORD\", \"Fake3Trail4Op5\")",
                "password =\n    ***REDACTED***\"***REDACTED***\", \"***REDACTED***\")",
            ),
            // The line ends with a continuation operator.
            (
                "const token =\n  process.env.TOKEN ||\n  \"Fake3Trail4Op5\";",
                "const token =\n  ***REDACTED*** ||\n  \"***REDACTED***\";",
            ),
            (
                "const token = ok ?\n  \"Fake3Trail4Op5\" :\n  \"x\";\nlabel = \"kept\"",
                "const token = ***REDACTED*** ?\n  \"***REDACTED***\" :\n  \"***REDACTED***\";\nlabel = \"kept\"",
            ),
            (
                "password = salt +\n    \"Fake3Trail4Op5\"",
                "password = ***REDACTED*** +\n    \"***REDACTED***\"",
            ),
            (
                "token := os.Getenv(\"TOKEN\").\n\tOr(\"Fake3Trail4Op5\")",
                "token :***REDACTED*** os.Getenv(\"***REDACTED***\").\n\tOr(\"***REDACTED***\")",
            ),
            (
                "$password = $salt .\n    \"Fake3Trail4Op5\";",
                "$password = ***REDACTED*** .\n    \"***REDACTED***\";",
            ),
            (
                "+const token =\n+  process.env.TOKEN ||\n+  \"Fake3Trail4Op5\";",
                "+const token =\n***REDACTED***  process.env.TOKEN ||\n+  \"***REDACTED***\";",
            ),
            // The same layout printed inside a `{:?}` string.
            (
                r#"left: "const token =\n  x ||\n  \"Fake3Trail4Op5\";""#,
                r#"left: "const token =***REDACTED***  x ||\n  \"***REDACTED***\";""#,
            ),
        ] {
            assert_both_mask(raw, "Fake3Trail4Op5", expected);
        }
        // A sentence's `.` is no chained call: what follows the break is kept.
        for (raw, kept) in [
            (
                "token: abc.\nAnd then she said \"hello\" twice.",
                "\"hello\"",
            ),
            ("token = \"Fake3Trail4Op5\"\nname = \"kept\"", "\"kept\""),
        ] {
            let stored = redact_stored_text(raw);
            assert!(!stored.contains("Fake3Trail4Op5"), "{stored}");
            assert!(stored.contains(kept), "{raw:?} -> {stored:?}");
            assert!(redact_agent_output(raw).contains(kept), "{raw:?}");
        }
    }

    #[test]
    fn a_trailing_comment_does_not_hide_the_continuation_operator() {
        // Stored text: the bare pattern of `ASSIGNMENT` also masks the code
        // before the operator, as for the round-3 shapes (fail-closed).
        for (raw, expected) in [
            (
                "const token = process.env.TOKEN || // default\n  \"Fake4Comment5Op6\";",
                "const token = ***REDACTED*** || // default\n  \"***REDACTED***\";",
            ),
            (
                "const token = process.env.TOKEN ?? // dev only\n  \"Fake4Comment5Op6\";",
                "const token = ***REDACTED*** ?? // dev only\n  \"***REDACTED***\";",
            ),
            (
                "password: process.env.P || // local\n  \"Fake4Comment5Op6\",\nuser: \"kept\"",
                "password: ***REDACTED*** || // local\n  \"***REDACTED***\",\nuser: \"kept\"",
            ),
            (
                "const token = process.env.TOKEN || /* default */\n  \"Fake4Comment5Op6\";",
                "const token = ***REDACTED*** || /* default */\n  \"***REDACTED***\";",
            ),
            (
                "token = ENV[\"T\"] || # fallback\n  \"Fake4Comment5Op6\"",
                "token = ***REDACTED***\"***REDACTED***\"] || # fallback\n  \"***REDACTED***\"",
            ),
            (
                "my $password = $ENV{X} //\n  \"Fake4Comment5Op6\";",
                "my $password = ***REDACTED*** //\n  \"***REDACTED***\";",
            ),
        ] {
            assert_both_mask(raw, "Fake4Comment5Op6", expected);
            assert!(
                redact_agent_output(raw).lines().next() == raw.lines().next(),
                "agent output keeps the code line: {raw:?}"
            );
        }
        // A comment after a word carries nothing: what follows is kept.
        for raw in [
            "token = \"Fake4Comment5Op6\" // default\nname = \"kept\"",
            "token = \"Fake4Comment5Op6\" # fallback ||\nname = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Comment5"), "{raw:?} -> {out:?}");
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_diff_is_read_in_its_old_and_new_versions() {
        let text = " a ||\n-  \"x\";\n+  \"y\";\n b";
        let [old, new] = diff_versions(text).expect("a diff");
        assert_eq!(old, format!(" a ||\n   \"x\";\n{}\n b", " ".repeat(7)));
        assert_eq!(new, format!(" a ||\n{}\n   \"y\";\n b", " ".repeat(7)));
        assert!(diff_versions(" a ||\n  \"x\"").is_none());
        assert_eq!(
            diff_versions("+é\r\n-ü").expect("a diff"),
            ["    \n ü".to_string(), " é\r\n   ".to_string()]
        );
    }

    /// Asserts that no half of `secret` survives `raw` through either entry
    /// point, and that a second pass changes neither output.
    fn assert_both_hide(raw: &str, secret: &str) {
        for (pass, redact) in [
            ("stored", redact_stored_text as fn(&str) -> String),
            ("agent", redact_agent_output),
        ] {
            let out = redact(raw);
            for part in [&secret[secret.len() / 2..], &secret[..secret.len() / 2]] {
                assert!(
                    !out.contains(part),
                    "{pass}: {part} leaked: {raw:?} -> {out:?}"
                );
            }
            assert_eq!(redact(&out), out, "{pass} idempotent: {raw:?}");
        }
    }

    #[test]
    fn a_changed_continuation_line_of_a_diff_is_read_in_both_versions() {
        // KT-2053 round 5: the carry that enters a `-` run enters the `+` run
        // after it, and a context line after both continues from either.
        for raw in [
            " const token = a ||\n-  \"x\";\n+  \"Fake5Diff6Op7\";",
            " const token =\n-  \"x\";\n+  \"Fake5Diff6Op7\";",
            "@@ -1,2 +1,2 @@\n token = a ||\n-  \"x\"\n+  \"Fake5Diff6Op7\"",
            " token = a ||\n-  // old\n+  // new\n   \"Fake5Diff6Op7\"",
            " token = Some(\n-    \"x\"),\n+    \"Fake5Diff6Op7\"),",
            " token = a ||\n-  b ||\n+  c\n   \"Fake5Diff6Op7\"",
            " token = a ||\n+  b ||\n-  c\n   \"Fake5Diff6Op7\"",
            " token = a ||\n-  \"x\";\n   \"Fake5Diff6Op7\";",
            "-token = a ||\n+token = b ||\n   \"Fake5Diff6Op7\"",
        ] {
            assert_both_hide(raw, "Fake5Diff6Op7");
        }
        // A value that ends in both versions keeps the line after it.
        for raw in [
            "+token = \"Fake5Diff6Op7\"\n+name = \"kept\"",
            "-token = \"x\"\n+token = \"Fake5Diff6Op7\"\n name = \"kept\"",
            " token = a ||\n-  \"x\";\n+  \"Fake5Diff6Op7\";\n name = \"kept\"",
        ] {
            assert_both_hide(raw, "Fake5Diff6Op7");
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_word_operator_or_a_caret_at_the_end_of_a_line_carries_it() {
        for raw in [
            "local token = os.getenv(\"T\") or\n  \"Fake5Word6Op7\"",
            "local token = os.getenv(\"T\") and\n  \"Fake5Word6Op7\"",
            "local token = os.getenv(\"T\") OR\n  \"Fake5Word6Op7\"",
            "local token = \"sk_\" ..\n  \"Fake5Word6Op7\"",
            "token = a or  # default\n  \"Fake5Word6Op7\"",
            "set token=%A% ^\n  \"Fake5Word6Op7\"",
        ] {
            assert_both_hide(raw, "Fake5Word6Op7");
        }
        // A word that merely ends in `or` / `and` carries nothing.
        for raw in [
            "token = \"Fake5Word6Op7\" + color\nname = \"kept\"",
            "token = \"Fake5Word6Op7\" + brand\nname = \"kept\"",
            "token = \"Fake5Word6Op7\"..\nname = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Word6"), "{raw:?} -> {out:?}");
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_dash_dash_or_hash_comment_after_an_operator_does_not_hide_it() {
        for raw in [
            "local token = os.getenv(\"T\") or -- default\n  \"Fake5Dash6Op7\"",
            "local token = os.getenv(\"T\") or --[[ default ]]\n  \"Fake5Dash6Op7\"",
            "local token = os.getenv(\"T\") or --[==[ a ]] b ]==]\n  \"Fake5Dash6Op7\"",
            "local token = os.getenv(\"T\") or --[[ a\n b ]]\n  \"Fake5Dash6Op7\"",
            "password = 'a' || -- c\n  'Fake5Dash6Op7'",
            "token = a ?? -- c\n  \"Fake5Dash6Op7\"",
            "token = a .. -- c\n  \"Fake5Dash6Op7\"",
            "token = a || ## c\n  \"Fake5Dash6Op7\"",
            "token = a || ##\n  \"Fake5Dash6Op7\"",
            "token = a || #!c\n  \"Fake5Dash6Op7\"",
            "token = a || #[c]\n  \"Fake5Dash6Op7\"",
            "token = a || #\"c\n  \"Fake5Dash6Op7\"",
            // A raw `#"…"#` closes only at `"#`, or runs to the end.
            "token = #\"Fake5Dash6Op7\"#, name = \"kept\"",
            "token = #\"Fake5\"Dash6Op7\"#",
            "token = #\"Fake5\"Dash6Op7",
        ] {
            assert_both_hide(raw, "Fake5Dash6Op7");
        }
        // Neither a decrement nor an attribute after a word is a comment.
        for raw in [
            "token = \"Fake5Dash6Op7\" + i--, name = \"kept\"",
            "token = \"Fake5Dash6Op7\" #[x], name = \"kept\"",
            "token = #\"Fake5Dash6Op7\"#, name = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Dash6"), "{raw:?} -> {out:?}");
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_word_operator_dot_dot_or_slash_slash_starting_the_next_line_carries_it() {
        // KT-2053 round 6: the mirror of the line-end word operators, the
        // usual Lua and Perl layout of a fallback.
        for raw in [
            "local token = os.getenv(\"T\")\n  or \"Fake6Lead7Op8\"",
            "local token = os.getenv(\"T\")\n  and \"Fake6Lead7Op8\"",
            "local token = os.getenv(\"T\")\n  OR \"Fake6Lead7Op8\"",
            "token = a\n  or \"Fake6Lead7Op8\"",
            "my $token = $ENV{X}\n  or \"Fake6Lead7Op8\";",
            "local token = \"sk_\"\n  .. \"Fake6Lead7Op8\"",
            "local token = \"sk_\"\n  ..\"Fake6Lead7Op8\"",
            "my $token = $ENV{X}\n  // \"Fake6Lead7Op8\";",
            "local token = os.getenv(\"T\") -- c\n  or \"Fake6Lead7Op8\"",
            "local token = os.getenv(\"T\") // c\n  or \"Fake6Lead7Op8\"",
            " local token = os.getenv(\"T\")\n-  or \"x\"\n+  or \"Fake6Lead7Op8\"",
            " local token = os.getenv(\"T\")\n+  or \"Fake6Lead7Op8\"",
            "+local token = \"sk_\"\n+  .. \"Fake6Lead7Op8\"",
            "local token = os.getenv(\"T\")\n  or(\"Fake6Lead7Op8\")",
            "token = a\n  Or \"Fake6Lead7Op8\"",
        ] {
            assert_both_hide(raw, "Fake6Lead7Op8");
        }
        // A word that only starts like one, a `...` and a line that starts
        // with anything else still end the value at the break.
        for raw in [
            "token = \"Fake6Lead7Op8\"\norder = \"kept\"",
            "token = \"Fake6Lead7Op8\"\nandroid = \"kept\"",
            "token = \"Fake6Lead7Op8\"\n...more \"kept\"",
            "token = \"Fake6Lead7Op8\"\nname = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Lead7"), "{raw:?} -> {out:?}");
                assert!(out.contains("\"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_comment_only_line_before_a_leading_operator_is_skipped_like_a_blank_one() {
        // KT-2053 round 7: the mirror of round 5's `a ||\n  // note\n  "…"`.
        for raw in [
            "local token = os.getenv(\"T\")\n  -- a comment line\n  or \"Fake7Note8Line9\"",
            "token = x\n  # c\n  or \"Fake7Note8Line9\"",
            "const token = a\n  /* c */\n  || \"Fake7Note8Line9\";",
            "token = x -- c\n  -- d\n  or \"Fake7Note8Line9\"",
            "{\"code\": \"local token = a\\n  -- c\\n  or \\\"Fake7Note8Line9\\\"\"}",
            // Comments spanning lines, stacked, after a blank line, or two on
            // one line; any `#`, and a comment at the start of the line.
            "token = x\n  --[[ a\n  b ]]\n  or \"Fake7Note8Line9\"",
            "token = x\n  --[==[ a ]]\n  ]==]\n  or \"Fake7Note8Line9\"",
            "token = x\n  /* a\n   * b\n   */\n  or \"Fake7Note8Line9\"",
            "token = x\n  # a\n\n  -- b\n  // c\n  or \"Fake7Note8Line9\"",
            "token = x\n  /* a */ -- b\n  or \"Fake7Note8Line9\"",
            "token = x\n  /* a */ or \"Fake7Note8Line9\"",
            "token = x\n  #[c]\n  or \"Fake7Note8Line9\"",
            "token = x\n-- c\nor \"Fake7Note8Line9\"",
            "token = x\r\n  # c\r\n  or \"Fake7Note8Line9\"",
            // In a diff's versions.
            " token = x\n-  -- old\n+  -- new\n   or \"Fake7Note8Line9\"",
            " token = x\n+  # c\n+  or \"Fake7Note8Line9\"",
            "+token = x\n+  -- c\n+  or \"Fake7Note8Line9\"",
            " token = x\n-  # c\n-  or \"y\"\n+  # c\n+  or \"Fake7Note8Line9\"",
            // A literal inside a comment line read so is masked too.
            "token = x\n  -- was \"Fake7Note8Line9\"\n  or y",
        ] {
            assert_both_hide(raw, "Fake7Note8Line9");
        }
        // A comment-only line before anything but a leading operator still
        // ends the value at the break.
        for raw in [
            "token = \"Fake7Note8Line9\"\n# c\nname = \"kept\"",
            "token = \"Fake7Note8Line9\"\n-- c\nname = \"kept\"",
            "token = \"Fake7Note8Line9\"\n/* c */\nname = \"kept\"",
            "token = \"Fake7Note8Line9\"\n/* a\n b */\nname = \"kept\"",
            "token = \"Fake7Note8Line9\"\n  // c\n  # d\nname = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Note8"), "{raw:?} -> {out:?}");
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_lua_long_bracket_string_is_a_literal_of_the_value() {
        // KT-2053 round 7: `[[…]]` and `[=[…]=]` (level-matched).
        for raw in [
            "token = [[Fake7Long8Br9]]",
            "token = x or [[Fake7Long8Br9]]",
            "token = x\n  or [[Fake7Long8Br9]]",
            "local token = os.getenv(\"T\") or [==[Fake7Long8Br9]==]",
            "local token = [=[Fake7Long8Br9]]x]=], name = \"kept\"",
            "local token = x .. [[Fake7Long8Br9]] .. y",
            "{\"code\": \"local token = x or [[Fake7Long8Br9]]\"}",
            // Not closed (or closed at another level): to the end of the text.
            "token = [[Fake7Long8Br9",
            "local token = os.getenv(\"T\") or [==[Fake7Long8Br9]=]",
        ] {
            assert_both_hide(raw, "Fake7Long8Br9");
        }
        // A body across lines (agent mode keeps a value with blanks).
        let raw = "local token = [[Fake7Long8Br9\nSecond5Line6]]\nname = \"kept\"";
        let out = redact_stored_text(raw);
        assert!(!out.contains("Long8") && !out.contains("Line6"), "{out:?}");
        assert!(out.contains("name = \"kept\""), "{out:?}");
        // The expression forms are masked by the value-expression pass itself.
        for raw in [
            "token = x or [[Fake7Long8Br9]]",
            "token = x\n  or [[Fake7Long8Br9]]",
            "local token = os.getenv(\"T\") or [==[Fake7Long8Br9]==]",
        ] {
            let (out, masked) = mask_value_expressions(raw, &|_: &str, _: &str| true);
            assert!(masked > 0 && !out.contains("Long8"), "{raw:?} -> {out:?}");
        }
        // A `[[` after a name or a closer is an index, not a literal.
        for raw in [
            "token = a[[1]], name = \"kept\"",
            "token = f(x)[[1]], name = \"kept\"",
        ] {
            let (out, masked) = mask_value_expressions(raw, &|_: &str, _: &str| true);
            assert_eq!((out.as_str(), masked), (raw, 0), "{raw:?}");
        }
    }

    // ── KT-2053 second security review (round 8) ──────────────────────

    #[test]
    fn the_second_security_review_inputs_mask_the_secret_and_keep_the_code() {
        // The exact inputs of findings 1-4 and 6: stored output pinned (the
        // bare-value pattern still masks the first token after the `:` / `=`,
        // as before), and agent output keeps every byte but the secret's.
        for (raw, stored) in [
            // 1: a generic left open at a line break.
            (
                "let api_key: Option<\n    String,\n> = Some(\"S3cr3tVal-99\".to_string());",
                "let api_key: ***REDACTED***\n    String,\n> = Some(\"***REDACTED***\".to_string());",
            ),
            (
                "const api_key: Record<\n  string,\n  string\n> = { key: \"S3cr3tVal-99\" };",
                "const api_key: ***REDACTED***\n  string,\n  string\n> = { key: \"***REDACTED***\" };",
            ),
            (
                "let api_key = env::var::<\n    &str,\n>(\"API_KEY\").unwrap_or(\"S3cr3tVal-99\");",
                "let api_key = ***REDACTED***\n    &str,\n>(\"***REDACTED***\").unwrap_or(\"***REDACTED***\");",
            ),
            // 2: the `>` of `->` / `=>`.
            (
                "let token: Result<fn() -> String, Error> = Ok(\"S3cr3tVal-99\".to_string());",
                "let token: ***REDACTED*** -> String, Error> = Ok(\"***REDACTED***\".to_string());",
            ),
            (
                "const token: Map<() => void, string> = new Map([[fn, \"S3cr3tVal-99\"]]);",
                "const token: ***REDACTED*** => void, string> = new Map([[***REDACTED***]]);",
            ),
            // 3: a quote after an operator `.` / `?`.
            (
                "$api_key = $prefix.\":\".\"S3cr3tVal-99\";",
                "$api_key = ***REDACTED***\"***REDACTED***\".\"***REDACTED***\";",
            ),
            (
                "const token = empty?\" \":\"S3cr3tVal-99\";",
                "const token = ***REDACTED***\"***REDACTED***\":\"***REDACTED***\";",
            ),
            (
                "const password = env.PASS??\":S3cr3tVal-99\";",
                "const password = ***REDACTED***\"***REDACTED***\";",
            ),
            // 4: a comment glued to `:` / `(`.
            (
                "const token = cond ? a :/* fallback; */ \"S3cr3tVal-99\";",
                "const token = ***REDACTED*** ? a :/* fallback; */ \"***REDACTED***\";",
            ),
            (
                "const token = cond ? a :/* fallback */\n  \"S3cr3tVal-99\";",
                "const token = ***REDACTED*** ? a :/* fallback */\n  \"***REDACTED***\";",
            ),
            (
                "const token = (// 1) fallback\n  \"S3cr3tVal-99\"\n);",
                "const token = ***REDACTED*** 1) fallback\n  \"***REDACTED***\"\n);",
            ),
            // 6: a comment's unclosed literal stops at its line end.
            (
                "token = os.getenv(\"X\") or # don't use \"old\n  fallback(\"arg1\", \"S3cr3tVal-99\")",
                "token = ***REDACTED***\"***REDACTED***\") or # don't use \"***REDACTED***\n  fallback(\"***REDACTED***\", \"***REDACTED***\")",
            ),
        ] {
            assert_both_mask(raw, "S3cr3tVal-99", stored);
            // The whole body is masked: `":S3cr3tVal-99"` loses its `:` too.
            assert_eq!(
                redact_agent_output(raw),
                raw.replace("\":S3cr3tVal-99", "\"S3cr3tVal-99")
                    .replace("S3cr3tVal-99", "***REDACTED***"),
                "agent: {raw:?}"
            );
        }
    }

    #[test]
    fn a_generic_left_open_at_a_line_break_still_holds_the_value() {
        // Finding 1: a line break no longer closes an open generic when the
        // type list goes on (`Option<\n`, `String,\n`, `\n>`).
        for raw in [
            "let api_key: Option<\n    String,\n> = Some(\"S3cr3tVal-99\".to_string());",
            "const api_key: Record<\n  string,\n  string\n> = { key: \"S3cr3tVal-99\" };",
            "let api_key = env::var::<\n    &str,\n>(\"API_KEY\").unwrap_or(\"S3cr3tVal-99\");",
            "let api_key: HashMap<\n  String,\n  Vec<\n    u8,\n  >,\n> = f(\"S3cr3tVal-99\");",
            "let api_key: Option<\r\n    String,\r\n> = Some(\"S3cr3tVal-99\");",
            "let api_key: Foo<\n    'a,\n> = Foo(\"S3cr3tVal-99\");",
            "{\"code\": \"let api_key: Option<\\n    String,\\n> = Some(\\\"S3cr3tVal-99\\\");\"}",
            " let api_key: Option<\n-    String,\n+    Box<str>,\n > = Some(\"S3cr3tVal-99\");",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
        // A comparison glued to a word at the end of a line opens no value
        // that never ends: the next line that is not part of a type list is
        // read as before.
        for raw in [
            "token = a<b\nname = \"kept\"",
            "token = a < b\nname = \"kept\"",
            "token = \"S3cr3tVal-99\" if a<b\nname = \"kept\"",
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Val-99"), "{raw:?} -> {out:?}");
                assert!(out.contains("name = \"kept\""), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn an_arrow_inside_a_generic_does_not_close_it() {
        // Finding 2: the `>` of `->` / `=>` is no generic's closer.
        for raw in [
            "let token: Result<fn() -> String, Error> = Ok(\"S3cr3tVal-99\".to_string());",
            "const token: Map<() => void, string> = new Map([[fn, \"S3cr3tVal-99\"]]);",
            "let token: Box<dyn Fn(u8) -> u8, A> = mk(\"S3cr3tVal-99\");",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
    }

    #[test]
    fn a_quote_after_an_operator_dot_or_question_mark_opens_a_literal() {
        // Finding 3: a `.` after a `$var` or a closer, `??`, a ternary `?`
        // are operators, not a sentence's end.
        for raw in [
            "$api_key = $prefix.\":\".\"S3cr3tVal-99\";",
            "const token = empty?\" \":\"S3cr3tVal-99\";",
            "const password = env.PASS??\":S3cr3tVal-99\";",
            "$api_key = $this->prefix.\":\".\"S3cr3tVal-99\";",
            "$api_key = self::P.\" \".\"S3cr3tVal-99\";",
            "$api_key = f(). \", \" .\"S3cr3tVal-99\";",
            "local token = \"a\"..\" \"..\"S3cr3tVal-99\"",
            "const token = !!\" S3cr3tVal-99\";",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
        // Prose still ends at its closing quote.
        for (raw, kept) in [
            (
                r#"{"a": "the password is X and the key Y.", "b": "c"}"#,
                r#", "b": "c"}"#,
            ),
            (
                r#"{"a": "token: see the doc (above).", "b": "c"}"#,
                r#", "b": "c"}"#,
            ),
            (r#"{"a": "token: wait...", "b": "c"}"#, r#", "b": "c"}"#),
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(out.ends_with(kept), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_comment_glued_to_an_opener_does_not_hide_the_value() {
        // Finding 4: a comment right after `:` `(` `[` `{` `,` `^`.
        for raw in [
            "const token = cond ? a :/* fallback; */ \"S3cr3tVal-99\";",
            "const token = cond ? a :/* fallback */\n  \"S3cr3tVal-99\";",
            "const token = (// 1) fallback\n  \"S3cr3tVal-99\"\n);",
            "const token = [/* a] */ \"S3cr3tVal-99\"];",
            "const token = f(a,// x) y\n  \"S3cr3tVal-99\");",
            "token = f(a,-- x) y\n  \"S3cr3tVal-99\")",
            "token = f(# x) y\n  \"S3cr3tVal-99\")",
            "set token=%A% ^// x\n  \"S3cr3tVal-99\"",
            "const token = cond ? a :// fallback\n  \"S3cr3tVal-99\";",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
        // Not comments: a URL, a decrement, an attribute, a raw string, a
        // CSS color.
        for (text, literals, rest) in [
            (" \"https://x\", y: 1", vec!["https://x"], ", y: 1"),
            (" url:https://x.y/z, k: \"b\"", vec![], ", k: \"b\""),
            (" f(--i), k: \"b\"", vec![], ", k: \"b\""),
            (" f(#[x], \"a\"), b: 1", vec!["a"], ", b: 1"),
            (" f(#\"a\"#), b: 1", vec!["a"], ", b: 1"),
            (" {color:#fff, x: \"a\"}, b: 1", vec!["a"], ", b: 1"),
        ] {
            let (found, end) = expression_literals(text, 0, &mut CutMemo::default());
            let found: Vec<&str> = found.into_iter().map(|range| &text[range]).collect();
            assert_eq!((found, &text[end..]), (literals, rest), "{text:?}");
        }
    }

    #[test]
    fn a_comment_literal_its_line_does_not_close_stops_at_the_line_end() {
        // Finding 6: the next line keeps its code and its line break.
        let raw =
            "token = os.getenv(\"X\") or # don't use \"old\n  fallback(\"arg1\", \"S3cr3tVal-99\")";
        for out in [redact_stored_text(raw), redact_agent_output(raw)] {
            assert!(!out.contains("Val-99"), "{out:?}");
            assert!(out.contains("\n  fallback(\""), "{out:?}");
        }
        assert_both_hide(raw, "S3cr3tVal-99");
    }

    // ── KT-2053 round 9: a `,` at the value's own depth ──────────────────

    #[test]
    fn a_top_level_comma_ends_the_value_only_before_a_new_key() {
        // G1: a Python tuple or a Ruby array assignment goes on past its `,`.
        for raw in [
            "token = x, \"S3cr3tVal-99\"",
            "api_key = prefix, \"S3cr3tVal-99\"",
            "token = x,\n  \"S3cr3tVal-99\"",
            "token = x,# c; d\n  \"S3cr3tVal-99\"",
            "token = x, \"S3cr3tVal-99\"\nname = \"kept\"",
            "token = x , 'S3cr3tVal-99'",
            "token = x,\r\n  \"S3cr3tVal-99\"",
            "token = x, # c\n  \"S3cr3tVal-99\"",
            "token = x, /* c */ \"S3cr3tVal-99\"",
            "token = x, y, z, \"S3cr3tVal-99\"",
            "token = |a, b| \"S3cr3tVal-99\"",
            "token = lambda a, b: \"S3cr3tVal-99\"",
            "token = x,\n  // c\n  \"S3cr3tVal-99\"",
            "{\"code\": \"token = x,\\n  \\\"S3cr3tVal-99\\\"\"}",
            " token = x,\n-  \"y\"\n+  \"S3cr3tVal-99\"",
            "+token = x,\n+  \"S3cr3tVal-99\"",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
        let raw = "token = x, \"S3cr3tVal-99\"\nname = \"kept\"";
        for out in [redact_stored_text(raw), redact_agent_output(raw)] {
            assert!(out.ends_with("\nname = \"kept\""), "{raw:?} -> {out:?}");
        }
        // A new key after the `,` ends the value: its own value is kept.
        for (raw, kept) in [
            (
                "connect(token = \"S3cr3tVal-99\", name = \"kept\")",
                ", name = \"kept\")",
            ),
            ("token=abc, user=\"bob\"", ", user=\"bob\""),
            (
                "{\"token\": \"S3cr3tVal-99\", \"name\": \"kept\"}",
                ", \"name\": \"kept\"}",
            ),
            (
                "{ token: 'S3cr3tVal-99', 'name': 'kept' }",
                ", 'name': 'kept' }",
            ),
            (
                "Cfg { token: Some(\"S3cr3tVal-99\"), retries: 3, name: \"kept\" }",
                ", retries: 3, name: \"kept\" }",
            ),
            (
                "connect(\n    token = \"S3cr3tVal-99\",\n    name = \"kept\",\n)",
                ",\n    name = \"kept\",\n)",
            ),
            (
                "{\n  \"token\": \"S3cr3tVal-99\",  # the token\n  \"name\": \"kept\"\n}",
                "\n  \"name\": \"kept\"\n}",
            ),
            (
                "{ token: \"S3cr3tVal-99\", :name => \"kept\" }",
                ", :name => \"kept\" }",
            ),
            (
                "['token' => 'S3cr3tVal-99', 'name' => 'kept']",
                ", 'name' => 'kept']",
            ),
            (
                "{ token: \"S3cr3tVal-99\", name?: \"kept\" }",
                ", name?: \"kept\" }",
            ),
            (
                "left: \"{\\\"token\\\": \\\"S3cr3tVal-99\\\", \\\"name\\\": \\\"kept\\\"}\"",
                ", \\\"name\\\": \\\"kept\\\"}\"",
            ),
            (
                " token = \"S3cr3tVal-99\",\n+  name = \"kept\",",
                "\n+  name = \"kept\",",
            ),
        ] {
            for out in [redact_stored_text(raw), redact_agent_output(raw)] {
                assert!(!out.contains("Val-99"), "{raw:?} -> {out:?}");
                assert!(out.ends_with(kept), "{raw:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn a_line_that_starts_with_a_comma_continues_the_value() {
        // G2: comma-first style, for an open generic and for the value.
        for raw in [
            "let api_key: HashMap<\n    String\n    , String\n> = make(\"S3cr3tVal-99\");",
            "let api_key: Result<String\n    , Error> = Ok(\"S3cr3tVal-99\");",
            "token = x\n  , \"S3cr3tVal-99\"",
            "let api_key = make(\n    a\n    , \"S3cr3tVal-99\"\n);",
            "token = x\n  -- c\n  , \"S3cr3tVal-99\"",
            "token = x\r\n  , \"S3cr3tVal-99\"",
            "{\"code\": \"token = x\\n  , \\\"S3cr3tVal-99\\\"\"}",
            " token = x\n-  , \"y\"\n+  , \"S3cr3tVal-99\"",
            " let api_key: Result<String\n+    , Error> = Ok(\"S3cr3tVal-99\");",
        ] {
            assert_both_hide(raw, "S3cr3tVal-99");
        }
        // A comma-first key still ends the value.
        let raw = "{ token = \"S3cr3tVal-99\"\n, name = \"kept\" }";
        for out in [redact_stored_text(raw), redact_agent_output(raw)] {
            assert!(!out.contains("Val-99"), "{out:?}");
            assert!(out.ends_with("\n, name = \"kept\" }"), "{out:?}");
        }
    }

    #[test]
    fn the_round_9_inputs_mask_the_secret_and_keep_the_code() {
        // The exact inputs of G1 and G2: stored output pinned, and agent
        // output keeps every byte but the secret's.
        // The bare-value pattern still masks the first token after the `=` /
        // `:` in stored text, as before this round.
        for (raw, stored) in [
            (
                "token = x, \"S3cr3tVal-99\"",
                "token = ***REDACTED***, \"***REDACTED***\"",
            ),
            (
                "api_key = prefix, \"S3cr3tVal-99\"",
                "api_key = ***REDACTED***, \"***REDACTED***\"",
            ),
            (
                "token = x,\n  \"S3cr3tVal-99\"",
                "token = ***REDACTED***,\n  \"***REDACTED***\"",
            ),
            (
                "token = x,# c; d\n  \"S3cr3tVal-99\"",
                "token = ***REDACTED***,# c; d\n  \"***REDACTED***\"",
            ),
            (
                "token = x, \"S3cr3tVal-99\"\nname = \"kept\"",
                "token = ***REDACTED***, \"***REDACTED***\"\nname = \"kept\"",
            ),
            (
                "let api_key: HashMap<\n    String\n    , String\n> = make(\"S3cr3tVal-99\");",
                "let api_key: ***REDACTED***\n    String\n    , String\n> = make(\"***REDACTED***\");",
            ),
            (
                "let api_key: Result<String\n    , Error> = Ok(\"S3cr3tVal-99\");",
                "let api_key: ***REDACTED***\n    , Error> = Ok(\"***REDACTED***\");",
            ),
            (
                "token = x\n  , \"S3cr3tVal-99\"",
                "token = ***REDACTED***\n  , \"***REDACTED***\"",
            ),
        ] {
            assert_both_mask(raw, "S3cr3tVal-99", stored);
            assert_eq!(
                redact_agent_output(raw),
                raw.replace("S3cr3tVal-99", "***REDACTED***"),
                "agent: {raw:?}"
            );
        }
    }

    #[test]
    fn a_comma_run_stays_linear() {
        // Round 9: each `,` looks ahead once, at the segment after it.
        for unit in [
            ",\n",
            "x,\n",
            "\n  , x",
            "a, b",
            "a, # c\n",
            "a, \"k\" ",
            "a, k ",
            "a,\r",
        ] {
            let text = format!("token = {}", unit.repeat(40_000));
            EXPRESSION_WORK.with(|work| work.set(0));
            mask_value_expressions_ungated(&text, &|_: &str, _: &str| true);
            let work = EXPRESSION_WORK.with(std::cell::Cell::get);
            assert!(
                work <= 16 * text.len(),
                "{unit:?}: {work} bytes read for {} bytes of text",
                text.len()
            );
        }
    }

    #[test]
    fn a_carriage_return_only_text_stays_linear() {
        // Finding 5: a `\r`-only line break looks back no further than its
        // own line.
        for unit in ["a ||\r", "a ||\r\n", "a ||\n", "a ||\r\r\n", "x\r or "] {
            let text = format!("token = {}", unit.repeat(40_000));
            EXPRESSION_WORK.with(|work| work.set(0));
            mask_value_expressions_ungated(&text, &|_: &str, _: &str| true);
            let work = EXPRESSION_WORK.with(std::cell::Cell::get);
            assert!(
                work <= 16 * text.len(),
                "{unit:?}: {work} bytes read for {} bytes of text",
                text.len()
            );
        }
    }

    #[test]
    fn the_value_expression_pass_stays_linear_on_adversarial_input() {
        let agent = |name: &str, value: &str| value_is_credential(name, value, true);
        let audit = |_: &str, _: &str| true;
        for unit in [
            // Unclosed raw literals, each name read again from inside the
            // literal before it (agent mode leaves them: they hold spaces).
            r#"token = r#\"a "#,
            r##"token = r#"a "##,
            r###"token = r##"a "###,
            // No whitespace, a placeholder start: left in agent mode too.
            r##"$a.token=r#""##,
            // Quotes after a `.`, read tentatively.
            r#"token = $x."a "#,
            r#"token = $x.\"a "#,
            // Escaped literals and bare quotes.
            r#"token: \"a\" " "#,
            // Line breaks looked past.
            "token = x\n\n\n",
            "+token = x\n+ ",
            // Line breaks carried by how the line ends (round 3).
            "token =\n",
            "token = x ||\n",
            "token = f().\nb(",
            "token = a .\n'a ",
            "+token =\n+ x ??\n",
            r#"token: \"a\" ||\n"#,
            // Trailing comments (round 4).
            "token = x || // x\n",
            "token = x /* */ ",
            "token = x /* ",
            "token = x // \"a \n",
            "token = x # y\n",
            // Diffs, word operators and `--` / `#` comments (round 5).
            "-token = a ||\n+  \"a \n",
            " token = a ||\n-  b ||\n+  c\n",
            "+token = (\n-x\n",
            "token = x or\n",
            "token = \"a\" ..\n",
            "token = x ^\n",
            "token = x or -- y\n",
            "token = x --[=[ ]=] ",
            "token = x --[==[ ]=] ",
            "token = x || ## \"a \n",
            "token = x || #\"a ",
            "token = x || # \"a ",
            // Leading word operators, `..` and `//` (round 6).
            "token = x\n or ",
            "token = x\n  .. \"a \n",
            "token = x\n // \"a \n",
            "token = x -- y\n or\n",
            " token = x\n-  or b\n+  or ",
            "token = x\n order\n",
            // Comment-only lines before a leading operator, and Lua long
            // brackets (round 7).
            "token = x\n -- c\n or ",
            "token = x\n # c\n",
            "token = x\n # token = x\n",
            "token = x\n /* c\n",
            "token = x\n /* c */\n",
            "token = x\n --[[ c\n",
            "token = x\n --[=[ c ]]\n",
            "token = [[a ",
            "token = [==[a ]=] ",
            "token = x or [=[a ",
            // Second security review (round 8): generics across lines,
            // arrows, operator quotes, glued comments, `\r`-only and mixed
            // line breaks, comment literals capped at their line.
            "token: A<\n B,\n",
            "token: A<\n B,\n> ",
            "token: A<\r B,\r",
            "token = a<b,\n",
            "token: A<fn() -> B, C> ",
            "token = $x.\":\".\"a ",
            "token = a?\" \":\"a ",
            "token = a??\": ",
            "token = a :/* ",
            "token = (// x\n",
            "token = a ||\r",
            "token = a\r or ",
            "token = a ||\r\n\r",
            "token = x || # \"a\n",
            // A `,` at the value's depth (round 9).
            "token = ,\n",
            "token = x,\n",
            "token = x\n  , x",
            "token = a, b",
            "token = a, # \"a\n",
            "token: A<B\n , C\n",
            "token = a, \"k\": ",
        ] {
            let text = unit.repeat(20_000);
            for masks in [&agent as &dyn Fn(&str, &str) -> bool, &audit] {
                EXPRESSION_WORK.with(|work| work.set(0));
                mask_value_expressions_ungated(&text, masks);
                let work = EXPRESSION_WORK.with(std::cell::Cell::get);
                assert!(
                    work <= 16 * text.len(),
                    "{unit:?}: {work} bytes read for {} bytes of text",
                    text.len()
                );
            }
        }
        // A run of blank lines before a continuation is crossed once.
        let text = format!("token = x{}.unwrap_or(\"Fake1\")", "\n".repeat(200_000));
        EXPRESSION_WORK.with(|work| work.set(0));
        let (out, _) = mask_value_expressions_ungated(&text, &audit);
        assert!(!out.contains("Fake1"));
        assert!(EXPRESSION_WORK.with(std::cell::Cell::get) <= 16 * text.len());
    }

    #[test]
    fn the_expression_reader_reads_literals_to_the_end_of_the_value() {
        let bodies = |text: &str| -> (Vec<String>, String) {
            let (literals, end) = expression_literals(text, 0, &mut CutMemo::default());
            (
                literals
                    .into_iter()
                    .map(|range| text[range].to_string())
                    .collect(),
                text[end..].to_string(),
            )
        };
        for (text, literals, rest) in [
            (r#" &str = "a", b: 1"#, vec!["a"], ", b: 1"),
            (r#" f("a", 'b'), c: 1"#, vec!["a", "b"], ", c: 1"),
            (r#" Vec<Box<str>>::new("a"); d"#, vec!["a"], "; d"),
            (r#" X::<&'static str>::new("a")}"#, vec!["a"], "}"),
            (r##" r#"a"b"#, c: 1"##, vec![r#"a"b"#], ", c: 1"),
            (r#" b"a" u'b' f`c`"#, vec!["a", "b", "c"], ""),
            (r#" Some(\"a\\\"b\") }"#, vec![r#"a\\\"b"#], "}"),
            (r#" abc" tail"#, vec![], r#"" tail"#),
            (r#" X and the key Y.", "b": "c""#, vec![], r#"", "b": "c""#),
            (r#" f(x) + "d""#, vec!["d"], ""),
            (" it's 'a'\nnext 'b'", vec!["a"], "\nnext 'b'"),
            (" Some(\n  \"a\",\n), x: 1", vec!["a"], ", x: 1"),
            (r#" Some(\n  \"a\"\n)\nnext"#, vec!["a"], r#"\nnext"#),
            (r#" "open"#, vec!["open"], ""),
            (r#" \"cut"#, vec!["cut"], ""),
            (r#" \"cut" tail"#, vec!["cut"], r#"" tail"#),
            // KT-2053 round 2: a quote after `.` `!` `?` opens a value in code.
            (r#" $salt."a1"; x"#, vec!["a1"], "; x"),
            (" 'sk_'.'a1'; x", vec!["sk_", "a1"], "; x"),
            (r#" ok?"a1":"b2"; x"#, vec!["a1", "b2"], "; x"),
            (r#" env.TOKEN??"a1"; x"#, vec!["a1"], "; x"),
            (r#" !!"a1""#, vec!["a1"], ""),
            (r#" x.\"a1\"; y"#, vec!["a1"], "; y"),
            (r#" x."open"#, vec!["open"], ""),
            // …and closes a sentence in prose.
            (r#" X and Y.", "b": "c""#, vec![], r#"", "b": "c""#),
            (r#" X and Y."}, {"b": 1}"#, vec![], r#""}, {"b": 1}"#),
            (r#" X and Y."#, vec![], ""),
            (" X and Y.\"\nnext \"q\"", vec![], "\"\nnext \"q\""),
            (r#" X and Y!\", \"b\""#, vec![], r#"\", \"b\""#),
            (" it's done.' and 'kept'", vec!["kept"], ""),
            // A line continued by a leading operator.
            (
                " env::var(\"T\")\n    .unwrap_or(\"a1\");\nnext",
                vec!["T", "a1"],
                ";\nnext",
            ),
            (" process.env.T\n  || \"a1\";", vec!["a1"], ";"),
            (" a\n\n  ?? \"a1\"\nnext \"b\"", vec!["a1"], "\nnext \"b\""),
            (
                " ok\n  ? \"a1\"\n  : \"b2\"\nnext",
                vec!["a1", "b2"],
                "\nnext",
            ),
            (" \"a\"\n  + \"b\"", vec!["a", "b"], ""),
            (" x\n  ?.y(\"a1\")", vec!["a1"], ""),
            (" x\n  && \"a1\"", vec!["a1"], ""),
            (r#" f(\"T\")\n    .or(\"a1\")"#, vec!["T", "a1"], ""),
            // …a line that starts anything else ends it.
            (" \"a\"\n...more \"b\"", vec!["a"], "\n...more \"b\""),
            (" \"a\"\n::path \"b\"", vec!["a"], "\n::path \"b\""),
            (" \"a\"\n| x \"b\"", vec!["a"], "\n| x \"b\""),
            (" \"a\"\n  next \"b\"", vec!["a"], "\n  next \"b\""),
            // In a diff, the next line's marker is not an operator.
            (
                "+token = \"a\"\n+name = \"b\"",
                vec!["a"],
                "\n+name = \"b\"",
            ),
            ("+let t = f(\"a\")\n+    .or(\"b\");", vec!["a", "b"], ";"),
            ("-let t = \"a\"\n-  + \"b\"", vec!["a", "b"], ""),
            // A bare `"` after an escaped literal closes the printed string.
            (
                r#" \"a1\""\n right: "kept""#,
                vec!["a1"],
                r#""\n right: "kept""#,
            ),
            (r#" Some(\"a1\") "tail" x"#, vec!["a1"], r#""tail" x"#),
            // KT-2053 round 3: nothing read since the `=` yet, the value is
            // on the next non-blank line…
            ("\n  process.env.T ?? \"a1\";", vec!["a1"], ";"),
            (
                "\n\n    os.getenv(\"T\", \"a1\")\nnext \"b\"",
                vec!["T", "a1"],
                "\nnext \"b\"",
            ),
            (" \t\n  \"a1\"", vec!["a1"], ""),
            (r#"\n  \"a1\"\n"#, vec!["a1"], r#"\n"#),
            ("\n+  \"a1\"", vec!["a1"], ""),
            // …or the line ends with a continuation operator.
            (" x ||\n  \"a1\";", vec!["a1"], ";"),
            (" x &&  \n  \"a1\"", vec!["a1"], ""),
            (" x ??\n\"a1\"", vec!["a1"], ""),
            (" ok ?\n  \"a1\" :\n  \"b2\";", vec!["a1", "b2"], ";"),
            (
                " salt +\n    \"a1\"\nnext \"b\"",
                vec!["a1"],
                "\nnext \"b\"",
            ),
            (" x ==\n  \"a1\"", vec!["a1"], ""),
            (" f(\"T\").\n\tOr(\"a1\")", vec!["T", "a1"], ""),
            (" f(\"T\").\n\tchain_2(\"a1\")", vec!["T", "a1"], ""),
            (" $salt .\n    \"a1\";", vec!["a1"], ";"),
            (r#" x ||\n  \"a1\""#, vec!["a1"], ""),
            ("+x ||\n+  \"a1\"", vec!["a1"], ""),
            // A trailing `.` before anything but a chained call is a sentence.
            (" abc.\nAnd then \"b\"", vec![], "\nAnd then \"b\""),
            (" abc.\nOr \"b\"", vec![], "\nOr \"b\""),
            (" abc.\n(\"b\")", vec![], "\n(\"b\")"),
            (" abc\nnext \"b\"", vec![], "\nnext \"b\""),
            (" \"a\" \nnext \"b\"", vec!["a"], "\nnext \"b\""),
            // KT-2053 round 4: a trailing comment is blanks for how the line
            // ends…
            (" x || // default\n  \"a1\";", vec!["a1"], ";"),
            (" x ?? // dev only\n  \"a1\"", vec!["a1"], ""),
            (" x || /* default */\n  \"a1\"", vec!["a1"], ""),
            (" x ||/* d */ \n  \"a1\"", vec!["a1"], ""),
            (" ENV[\"T\"] || # fallback\n  \"a1\"", vec!["T", "a1"], ""),
            (r#" x || // d\n  \"a1\""#, vec!["a1"], ""),
            ("+x || // d\n+  \"a1\"", vec!["a1"], ""),
            (" x ||\n  // note\n  \"a1\"", vec!["a1"], ""),
            (" // note\n  \"a1\"", vec!["a1"], ""),
            // …its brackets, `,` and `;` end nothing…
            (" x || // e.g., dev (see) }\n  \"a1\";", vec!["a1"], ";"),
            (" f( /* ) */ \"a1\")", vec!["a1"], ""),
            (" x || /* a\n b */ \"a1\"", vec!["a1"], ""),
            // …a literal inside it is still read (Perl's `//` operator)…
            (" x // \"q\"\nnext \"b\"", vec!["q"], "\nnext \"b\""),
            (" $ENV{X} // \"a1\"; next", vec!["a1"], ""),
            // …and a `//` with nothing after it is Perl's operator.
            (" $ENV{X} //\n  \"a1\"", vec!["a1"], ""),
            (" $ENV{X} //  \n\"a1\"", vec!["a1"], ""),
            (" $ENV{X}//\n  \"a1\"", vec!["a1"], ""),
            // After a word, a comment does not carry the line…
            (" abc // note\nnext \"b\"", vec![], "\nnext \"b\""),
            (" abc # note ||\nnext \"b\"", vec![], "\nnext \"b\""),
            (" abc. /* x */\nAnd then \"b\"", vec![], "\nAnd then \"b\""),
            // …and neither a URL nor a raw string or attribute is a comment.
            (" see https://x\nnext \"b\"", vec![], "\nnext \"b\""),
            (" #\"a1\"#, x: 1", vec!["a1"], ", x: 1"),
            (" a #[x], b: 1", vec![], ", b: 1"),
            // KT-2053 round 5: a word operator, ` ..` or `^` ends a line that
            // goes on…
            (" x or\n  \"a1\"", vec!["a1"], ""),
            (" x AND\n  \"a1\";", vec!["a1"], ";"),
            (" \"sk_\" ..\n  \"a1\"", vec!["sk_", "a1"], ""),
            (" x or  # default\n  \"a1\"", vec!["a1"], ""),
            (" %A% ^\n  \"a1\"", vec!["a1"], ""),
            // …a word that only ends like one, or a `..` right after a
            // literal, does not.
            (" color\nnext \"b\"", vec![], "\nnext \"b\""),
            (" brand\nnext \"b\"", vec![], "\nnext \"b\""),
            (" \"a\"..\nnext \"b\"", vec!["a"], "\nnext \"b\""),
            // A `--` comment, and Lua's long `--[[ … ]]` one, are blanks…
            (" x or -- default\n  \"a1\"", vec!["a1"], ""),
            (" x or --[[ d ]]\n  \"a1\"", vec!["a1"], ""),
            (" x or --[==[ a ]] b ]==]\n  \"a1\"", vec!["a1"], ""),
            (" x --[[ a\n b, ]] \"a1\"", vec!["a1"], ""),
            (" x || -- e.g., (see)\n  \"a1\";", vec!["a1"], ";"),
            // …as is any `#` after an operator…
            (" x || ## c\n  \"a1\"", vec!["a1"], ""),
            (" x || ##\n  \"a1\"", vec!["a1"], ""),
            (" x || #!c\n  \"a1\"", vec!["a1"], ""),
            (" x || #[c]\n  \"a1\"", vec!["a1"], ""),
            // …a literal its comment line does not close is masked to the
            // end of that line (second security review), and the comment
            // still ends with its line…
            (" x || #\"c\n  \"a1\"", vec!["c", "a1"], ""),
            (" x // c \"q\nnext \"b\"", vec!["q"], "\nnext \"b\""),
            (" x || # \"q\r\n  \"a1\"", vec!["q", "a1"], ""),
            // …a bare `#"` is a raw string, closed only by `"#`…
            (" #\"a\"b\"#, x: 1", vec!["a\"b"], ", x: 1"),
            (" #\"open", vec!["open"], ""),
            // …and neither a decrement nor a shebang after a word is a comment.
            (" i--, x: 1", vec![], ", x: 1"),
            (" a #!x, b: 1", vec![], ", b: 1"),
            // KT-2053 round 6: a next line that starts with `or` / `and` (a
            // whole word, any case), `..` or `//` goes on…
            (" f(\"T\")\n  or \"a1\"", vec!["T", "a1"], ""),
            (" x\n  and \"a1\";", vec!["a1"], ";"),
            (" x\n  OR \"a1\"", vec!["a1"], ""),
            (" x\n  And(\"a1\")", vec!["a1"], ""),
            (" x\n\tor\n  \"a1\"", vec!["a1"], ""),
            (" \"sk_\"\n  .. \"a1\"", vec!["sk_", "a1"], ""),
            (" \"sk_\"\n  ..\"a1\"", vec!["sk_", "a1"], ""),
            // (a leading `//` line is also read as a comment, so its `;` ends
            // nothing; the line after it is judged on the code before it.)
            (" $ENV{X}\n  // \"a1\";", vec!["a1"], ""),
            (
                " $ENV{X}\n  // \"a1\";\nnext \"b\"",
                vec!["a1"],
                "\nnext \"b\"",
            ),
            (" x -- c\n  or \"a1\"", vec!["a1"], ""),
            (" x /* c */\n  or \"a1\"", vec!["a1"], ""),
            (" x # c\n  or \"a1\"", vec!["a1"], ""),
            (r#" x\n  or \"a1\""#, vec!["a1"], ""),
            ("+x\n+  or \"a1\"", vec!["a1"], ""),
            ("-x\n-  .. \"a1\"", vec!["a1"], ""),
            // …a word that only starts like one, or a `...`, does not.
            (" \"a\"\norder \"b\"", vec!["a"], "\norder \"b\""),
            (" \"a\"\nandroid \"b\"", vec!["a"], "\nandroid \"b\""),
            (" \"a\"\nor_else \"b\"", vec!["a"], "\nor_else \"b\""),
            (" \"a\"\n... \"b\"", vec!["a"], "\n... \"b\""),
            // A capitalised `Or` / `And` carries only before a non-letter…
            (" x\n  Or \"a1\"", vec!["a1"], ""),
            (" x\n  And $y", vec![], ""),
            // …a sentence that starts with one ends the value.
            (" abc\nAnd then \"b\"", vec![], "\nAnd then \"b\""),
            (" abc\nOr maybe \"b\"", vec![], "\nOr maybe \"b\""),
            // KT-2053 round 7: a comment-only line before a leading operator
            // is skipped like a blank line (its literals still read)…
            (
                " f(\"T\")\n  -- a comment line\n  or \"a1\"",
                vec!["T", "a1"],
                "",
            ),
            (" x\n  # c\n  or \"a1\"", vec!["a1"], ""),
            (" a\n  /* c */\n  || \"a1\";", vec!["a1"], ";"),
            (" x -- c\n  -- d\n  or \"a1\"", vec!["a1"], ""),
            (r#" a\n  -- c\n  or \"a1\"""#, vec!["a1"], "\""),
            (" x\n  --[[ a\n  b ]]\n  or \"a1\"", vec!["a1"], ""),
            (" x\n  /* a\n  b */\n  or \"a1\"", vec!["a1"], ""),
            (" x\n  # a\n\n  -- b\n  // c\n  or \"a1\"", vec!["a1"], ""),
            (" x\n  /* a */ -- b\n  or \"a1\"", vec!["a1"], ""),
            (" x\n  #[c, d]\n  or \"a1\"", vec!["a1"], ""),
            (" x\n  -- was \"q\"\n  or \"a1\"", vec!["q", "a1"], ""),
            ("+x\n+  -- c\n+  or \"a1\"", vec!["a1"], ""),
            (" x\r\n  # c\r\n  or \"a1\"", vec!["a1"], ""),
            // …and before anything else the value ends at the first break.
            (
                " \"a\"\n# c\nname = \"b\"",
                vec!["a"],
                "\n# c\nname = \"b\"",
            ),
            (
                " \"a\"\n-- c\nname = \"b\"",
                vec!["a"],
                "\n-- c\nname = \"b\"",
            ),
            (
                " \"a\"\n/* c */\nname = \"b\"",
                vec!["a"],
                "\n/* c */\nname = \"b\"",
            ),
            (
                " \"a\"\n/* a\n b */\nname = \"b\"",
                vec!["a"],
                "\n/* a\n b */\nname = \"b\"",
            ),
            (" x\n  # c\n", vec![], "\n  # c\n"),
            (" x\n  /* c", vec![], "\n  /* c"),
            // Lua's long brackets are literals, level-matched…
            (" [[a1]], b: 1", vec!["a1"], ", b: 1"),
            (" x or [[a1]]", vec!["a1"], ""),
            (" x\n  or [[a1]]", vec!["a1"], ""),
            (" x or [==[a]]b]==]; c", vec!["a]]b"], "; c"),
            (" [[a\nb]]\nnext", vec!["a\nb"], "\nnext"),
            (" [[open", vec!["open"], ""),
            (" [=[open]]", vec!["open]]"], ""),
            (" x .. [[a1]] .. y", vec!["a1"], ""),
            // …but a `[[` after a name or a closer is an index.
            (" a[[1]], b: 1", vec![], ", b: 1"),
            (" a[[\"q\"]], b: 1", vec!["q"], ", b: 1"),
            (" f(x)[[1]], b: 1", vec![], ", b: 1"),
            // Second security review: a generic's list across lines…
            (
                " Option<\n    String,\n> = Some(\"a\"); x",
                vec!["a"],
                "; x",
            ),
            (" R<\n  s,\n  t\n> = { k: \"a\" }; x", vec!["a"], "; x"),
            (" f::<\r\n  &str,\r\n>(\"a\"); x", vec!["a"], "; x"),
            (" Foo<\n  'a,\n> = F(\"a\"); x", vec!["a"], "; x"),
            // …but a `<` glued to a word at a line end opens nothing past it.
            (" a<b\nnext \"b\"", vec![], "\nnext \"b\""),
            // An arrow closes no generic.
            (" R<fn() -> S, E> = Ok(\"a\"); x", vec!["a"], "; x"),
            (" M<() => V, S> = m(\"a\"); x", vec!["a"], "; x"),
            // A `[[` read as a long bracket and as a nested array.
            (" m([[k, \"a\"]]); x", vec!["k, \"a\"", "a"], "; x"),
            // A quote after an operator `.` `?` opens a literal.
            (" $p.\":\".\"a\"; x", vec![":", "a"], "; x"),
            (" e?\" \":\"a\"; x", vec![" ", "a"], "; x"),
            (" e??\":a\"; x", vec![":a"], "; x"),
            (" \"a\"..\" \"..\"b\"", vec!["a", " ", "b"], ""),
            // A comment glued to `:` `(` `,`.
            (" c ? a :/* b; */ \"a\"; x", vec!["a"], "; x"),
            (" (// b) c\n  \"a\"\n); x", vec!["a"], "; x"),
            (" f(a,-- b) c\n  \"a\"); x", vec!["a"], "; x"),
            // Round 9: a `,` at the value's depth ends it before a new key
            // only…
            (" x, \"a\"; y", vec!["a"], "; y"),
            (" x,\n  \"a\"; y", vec!["a"], "; y"),
            (" x,# c; d\n  \"a\"; y", vec!["a"], "; y"),
            (" x, \"a\"\nname = \"b\"", vec!["a"], "\nname = \"b\""),
            (" |a, b| \"a\"; y", vec!["a"], "; y"),
            (" lambda a, b: \"a\"; y", vec!["a"], "; y"),
            (" \"a\", name = \"b\"", vec!["a"], ", name = \"b\""),
            (" \"a\", name=\"b\"", vec!["a"], ", name=\"b\""),
            (" \"a\", name: \"b\"", vec!["a"], ", name: \"b\""),
            (" \"a\", \"name\": \"b\"", vec!["a"], ", \"name\": \"b\""),
            (" \"a\", 'name': \"b\"", vec!["a"], ", 'name': \"b\""),
            (
                " \\\"a\\\", \\\"name\\\": 1",
                vec!["a"],
                ", \\\"name\\\": 1",
            ),
            (" \"a\", :name => 1", vec!["a"], ", :name => 1"),
            (" \"a\", name?: 1", vec!["a"], ", name?: 1"),
            (" \"a\",\n  name = 1", vec!["a"], ",\n  name = 1"),
            (" \"a\", # c\n  name = 1", vec!["a"], ", # c\n  name = 1"),
            // …not before a comparison, a path or a call.
            (" x, y == \"a\"; z", vec!["a"], "; z"),
            (" x, a::b(\"a\"); z", vec!["a"], "; z"),
            (" x, f(\"a\"); z", vec!["a"], "; z"),
            // A line that starts with a `,` continues the value and a generic.
            (" x\n  , \"a\"; y", vec!["a"], "; y"),
            (" H<\n  S\n  , T\n> = m(\"a\"); y", vec!["a"], "; y"),
            (" R<S\n  , E> = Ok(\"a\"); y", vec!["a"], "; y"),
            (" \"a\"\n, name = 1", vec!["a"], ", name = 1"),
        ] {
            let (found, after) = bodies(text);
            assert_eq!(found, literals, "{text:?}");
            assert_eq!(after, rest, "{text:?}");
        }
    }

    #[test]
    fn the_value_expression_gate_admits_every_new_shape() {
        for sample in [
            r#"let token: &str = "x";"#,
            r#"password = b"x""#,
            r#"password = os.getenv("X", "y")"#,
            r#"{"token": os.getenv("X")}"#,
            r#"{ \"token\": b'x' }"#,
            r#"'token' => 'x'"#,
            r#"api_key = env::var_or("KEY", "x")"#,
        ] {
            assert!(VALUE_EXPRESSION_NAME.is_match(sample), "{sample:?}");
            assert!(value_expression_gate(sample), "{sample:?}");
        }
    }

    #[test]
    fn an_nvidia_key_is_masked_by_its_vendor_shape() {
        let key = "nvapi-Fake0Nvidia1Key2Value3Abcdef4_xyz-5";
        for raw in [
            format!(
                "ApiKey {{ id: \"k1\", provider: \"nvidia\", value: \"{key}\", active: true }}"
            ),
            format!("NVIDIA {key}"),
            format!("[{key}]"),
        ] {
            for out in [redact_secrets(&raw), redact_stored_text(&raw)] {
                assert!(!out.contains("Fake0Nvidia1Key2"), "{raw:?} -> {out:?}");
                assert!(out.contains("nvapi-***REDACTED***"), "{raw:?} -> {out:?}");
            }
            assert!(looks_like_secret(&raw), "{raw:?}");
        }
        // A word that merely starts the same way is no key.
        assert_eq!(redact_secrets("the nvapi-docs page"), "the nvapi-docs page");
    }

    #[test]
    fn userinfo_of_any_scheme_is_masked_and_the_host_kept() {
        for (raw, secret, kept) in [
            (
                "could not reach rediss://default:Fake5Redis6Pass@cache.internal:6380/0",
                "Fake5Redis6Pass",
                "rediss://default:***REDACTED***@cache.internal:6380/0",
            ),
            (
                "mssql://sa:Fake5Mssql6Pass@db.internal:1433/app",
                "Fake5Mssql6Pass",
                "mssql://sa:***REDACTED***@db.internal:1433/app",
            ),
            (
                "jdbc:sqlserver://app:Fake5Jdbc6Pass@db.internal",
                "Fake5Jdbc6Pass",
                "@db.internal",
            ),
            (
                "ftp://deploy:Fake5Ftp6Pass@files.internal/x",
                "Fake5Ftp6Pass",
                "ftp://deploy:***REDACTED***@files.internal/x",
            ),
            // Redis' own form: no user, only a password.
            (
                "redis://:Fake5Redis6Pass@cache.internal:6379",
                "Fake5Redis6Pass",
                "@cache.internal:6379",
            ),
            (
                "clickhouse+native://u:Fake5Ch6Pass@ch.internal",
                "Fake5Ch6Pass",
                "@ch.internal",
            ),
        ] {
            assert_stored_masks(raw, secret, kept);
            assert!(!redact_secrets(raw).contains(secret), "{raw:?}");
        }
        // No userinfo: left as written.
        for plain in [
            "see https://example.com:8080/docs and ftp://files.internal/x",
            "jdbc:sqlserver://db.internal:1433;databaseName=app",
            "ssh://git@github.com/o/r.git",
        ] {
            assert_eq!(redact_stored_text(plain), plain);
        }
    }

    #[test]
    fn a_kronn_api_key_does_not_print_its_value_through_debug() {
        let key = crate::models::ApiKey {
            id: "k1".into(),
            name: "NVIDIA".into(),
            provider: "nvidia".into(),
            value: "FakeDebug0Secret1Value2".into(),
            active: true,
        };
        let config = crate::models::TokensConfig {
            anthropic: Some("FakeDebug0Secret1Legacy".into()),
            openai: None,
            google: None,
            keys: vec![key.clone()],
            disabled_overrides: Vec::new(),
        };
        for printed in [
            format!("{key:?}"),
            format!("{key:#?}"),
            format!("{:?}", Some(&key)),
            format!("{config:?}"),
        ] {
            assert!(!printed.contains("FakeDebug0Secret1"), "{printed}");
            assert!(printed.contains("***REDACTED***"), "{printed}");
            // What tells one key from another is still there.
            for kept in ["k1", "NVIDIA", "nvidia", "active: true"] {
                assert!(printed.contains(kept), "{kept} lost: {printed}");
            }
        }
    }

    #[test]
    fn the_kt_2039_shapes_keep_a_json_document_valid() {
        let document = serde_json::json!({
            "stdout": "api_key: Some(\"Fake1Some2Value3Abc\")\n{ 'x-api-key': 'Fake7Quoted8Key9' }",
            "url": "rediss://default:Fake5Redis6Pass@cache.internal:6380/0",
            "key": "nvapi-Fake0Nvidia1Key2Value3Abcdef4",
        })
        .to_string();
        let out = redact_stored_text(&document);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("still JSON");
        for secret in [
            "Fake1Some2Value3Abc",
            "Fake7Quoted8Key9",
            "Fake5Redis6Pass",
            "Fake0Nvidia1Key2",
        ] {
            assert!(!out.contains(secret), "{secret} leaked: {out}");
        }
        assert_eq!(
            parsed["url"],
            "rediss://default:***REDACTED***@cache.internal:6380/0"
        );
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
            "rediss://u:p@h",
            "Some(",
            "Secret::new(",
            "'x-api-key': '",
            "'",
            "(",
            // KT-2053
            "strdup (",
            "::<String>::new(",
            "::<",
            ">",
            "|| ",
            "move |_| ",
            "|",
            "\\n    ",
            "\\\"",
            "\\\\\\\"",
            "{ \\\"token\\\": '",
            "{ 'token': \\\"",
            // KT-2053 review: value expressions.
            ": &str = ",
            "Option<String> = ",
            "b\"",
            "r#\"",
            "\"#",
            "&'static ",
            "::<Vec<Box<str>>>::new(",
            ".getenv(",
            "\"KEY\", ",
            "' => ",
            ")",
            "}",
            "]",
            "<",
            "nvapi-",
            "sk-",
            "p8e-",
            "AIza",
            "ghp_",
            "gho_",
            "xoxb-",
            "eyJ",
            "AKIA",
            "ASIA",
            "github_pat_",
            "ya29.",
            "1//0",
            "SECRET_KEY",
            "TOKEN_VALUE",
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
                prop_assert!(
                    !VALUE_EXPRESSION_NAME.is_match(&input) || value_expression_gate(&input),
                    "value-expression gate holds back a match on {:?}", input
                );
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
                prop_assert_eq!(
                    mask_value_expressions(&input, &|_, _| true),
                    mask_value_expressions_ungated(&input, &|_, _| true)
                );
                // The whole audit-artifact pipeline, every pass chained.
                let (once, first) = apply_patterns_ungated(&input, PATTERNS.as_slice());
                let (expressed, middle) = mask_value_expressions_ungated(&once, &|_, _| true);
                let (twice, second) = apply_patterns_ungated(&expressed, ASSIGNMENT.as_slice());
                let changed = twice != input;
                prop_assert_eq!(
                    redact_for_audit_artifact(&input),
                    (twice, if changed { (first + middle + second).max(1) } else { 0 })
                );
                prop_assert_eq!(
                    looks_like_secret(&input),
                    PATTERNS.iter().any(|pat| pat.re.is_match(&input))
                );
            }
        }
    }
}
