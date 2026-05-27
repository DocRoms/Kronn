//! 0.8.7 — Anti-hallucination program, Stage 1 ("blinder les prompts").
//!
//! Two primitives live here:
//!
//! - **P1 — directive** ([`PREAMBLE`] / [`preamble_if_active`]): a sourcing
//!   discipline injected into every agent system prompt at the runner
//!   chokepoint. Plain-language in 0.8.7 (file:line / URL / user-confirmed);
//!   it will tighten to the formal `[src: …]` grammar once the Phase 2 spec
//!   (`backend/docs/conventions/agents-md-format-v1.md`) ships.
//! - **P2 — post-output linter** ([`lint_assertions`]): a heuristic that
//!   flags confident technical assertions left without any verifiable anchor.
//!   False positives are acceptable by design — they surface as a non-blocking
//!   UI pill, never a hard failure (that's the whole point of `warn` mode).
//!
//! ## Mode
//! A single global flag ([`current_mode`] / [`set_mode`]) drives the feature:
//! `off` (nothing), `warn` (P1 injected + P2 lints + pill, **non-blocking**),
//! `enforce` (warn + future P3 write-refusal, which is Phase 3 — in 0.8.7
//! `enforce` behaves like `warn`). It's a process-global rather than threaded
//! through the ~10 agent-spawn call sites: the mode is one global setting, so a
//! feature-flag accessor set at config load + on save keeps injection logic in
//! exactly one place and automatically covers every current and future surface.

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use ts_rs::TS;

// ─── Mode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AntiHallucMode {
    Off,
    Warn,
    Enforce,
}

impl AntiHallucMode {
    /// Parse leniently: unknown / empty strings fall back to `Warn` (the
    /// 0.8.7 rollout default — visible but non-blocking).
    pub fn from_str_lenient(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "enforce" => Self::Enforce,
            _ => Self::Warn,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Enforce => "enforce",
        }
    }

    /// True when P1/P2 should run (anything other than `off`).
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The default mode when nothing has been configured yet.
pub const DEFAULT_MODE_STR: &str = "warn";

/// Whether a raw string is one of the three accepted modes (used by the API
/// validator so an invalid value is rejected instead of silently coerced).
pub fn is_valid_mode(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "off" | "warn" | "enforce")
}

static MODE: OnceLock<RwLock<AntiHallucMode>> = OnceLock::new();

fn cell() -> &'static RwLock<AntiHallucMode> {
    MODE.get_or_init(|| RwLock::new(AntiHallucMode::Warn))
}

/// Update the global mode from a config string. Called once at startup after
/// the config loads, and again whenever the setting is saved.
pub fn set_mode(s: &str) {
    *cell().write().unwrap() = AntiHallucMode::from_str_lenient(s);
}

pub fn current_mode() -> AntiHallucMode {
    *cell().read().unwrap()
}

// ─── P1 — directive ──────────────────────────────────────────────────────

/// The sourcing-discipline directive injected into agent prompts.
///
/// Plain-language in 0.8.7 on purpose: it must work before the formal
/// provenance spec exists (Phase 2). Once the spec ships, tighten the wording
/// to point at the `[src: <type>: <ref>]` grammar + the public spec URL.
pub const PREAMBLE: &str = "\
=== ANTI-HALLUCINATION (pointer) ===

Before stating any non-trivial technical fact (file paths, function / API / \
config names, versions, behaviour, conventions), apply the cascade defined in \
`docs/AGENTS.md` § anti-hallucination of this project : read the code → read \
docs/ → official external doc → ask the user → never assert without proof. \
Attach `[src: file: <path>:<line>]`, `[src: url: <url>]`, or \
`[src: user:<identifier>:<date>: …]` to every assertion ; citations that escape \
the project root or point at non-existent paths are rejected as fabricated.";

/// Phase 2 spec embedded at compile time so it ships with the binary and
/// always matches the running anti-halluc semantics (no FS read, no drift).
/// Served by the `/api/conventions/agents-md-format-v1` endpoint and rendered
/// in-app from the Settings → Sourcing & Anti-hallucination section.
///
/// Located inside `backend/docs/` (not repo-root `docs/`) on purpose : the
/// Docker backend builder's build-context is `./backend`, so any file outside
/// `backend/` is not visible to `include_str!` during `cargo build`. Keeping
/// the canonical spec under `backend/` keeps the dev build and the Docker
/// build aligned with zero extra COPY plumbing.
pub const SPEC_AGENTS_MD_V1: &str = include_str!("../../docs/conventions/agents-md-format-v1.md");

/// The directive when the feature is active, else `None`.
pub fn preamble_if_active() -> Option<&'static str> {
    if current_mode().is_active() {
        Some(PREAMBLE)
    } else {
        None
    }
}

// ─── P2 — post-output linter ─────────────────────────────────────────────

/// One flagged sentence: a short excerpt + the cue that tripped the heuristic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct FlaggedSpan {
    /// A char-bounded excerpt of the flagged sentence (≤ [`SPAN_EXCERPT_MAX_CHARS`]).
    pub text: String,
    /// The claim cue that matched (helps the user judge the flag + tune later).
    pub reason: String,
}

/// What kind of source a `[src: …]` marker points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    File,
    Url,
    User,
    Commit,
    /// `api:` capture — treated like a URL (unchecked, no network at finalize).
    Api,
    /// A code comment — may point at a real file:line, so it is still
    /// existence-verified, but it is LOW trust (a comment is not authoritative).
    CodeComment,
    /// A derived guess — soft, never file-verified.
    Inferred,
    /// An unverified hypothesis — soft, never file-verified.
    Hypothesis,
    /// Model prior knowledge — the hallucination case. Always rejected.
    TrainingData,
    /// Forward-compat catch-all for unknown future types (unchecked).
    Other,
}

/// Result of mechanically verifying one `[src: …]` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    /// File exists (and the line/range is in bounds), or a trusted tier.
    Verified,
    /// The cited file does not exist under the project root.
    NotFound,
    /// The cited line / range is beyond the file's length.
    OutOfBounds,
    /// `[src:]` with no usable reference.
    EmptyRef,
    /// The path escapes the project root (jailed — never touched the FS).
    OutsideProject,
    /// Not machine-verifiable here (URL, commit, user-declared, or no project root).
    Unchecked,
    /// A `training-data` citation — model prior knowledge, refused as a source.
    Rejected,
}

impl SourceStatus {
    /// High-confidence "this citation is fabricated, wrong, or refused" — drives
    /// the red pill. Distinct from `Unchecked`, which is honestly "we don't know".
    pub fn is_fabricated(self) -> bool {
        matches!(
            self,
            Self::NotFound
                | Self::OutOfBounds
                | Self::EmptyRef
                | Self::OutsideProject
                | Self::Rejected
        )
    }
}

/// One extracted `[src: …]` marker plus its mechanical verdict.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct SourceCheck {
    /// Inner content of the marker (after `src:`), trimmed.
    pub raw: String,
    pub kind: SourceKind,
    pub status: SourceStatus,
    /// Human-readable reason (shown in the pill drawer).
    pub detail: String,
}

/// The lint result attached to an agent message.
///
/// Two independent signals:
/// - `unsourced_count` / `flagged_spans` — **niveau 0**, the cheap prose
///   heuristic (low confidence, lenient, may have false positives).
/// - `sources` / `fabricated_count` — **niveau 1**, mechanical verification of
///   every `[src: …]` the agent emitted (high confidence, ungameable).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct LintReport {
    pub unsourced_count: u32,
    pub flagged_spans: Vec<FlaggedSpan>,
    #[serde(default)]
    pub sources: Vec<SourceCheck>,
    #[serde(default)]
    pub fabricated_count: u32,
}

impl LintReport {
    pub fn empty() -> Self {
        Self {
            unsourced_count: 0,
            flagged_spans: Vec::new(),
            sources: Vec::new(),
            fabricated_count: 0,
        }
    }
    /// Nothing worth showing the user (no unsourced claims, no bad citations).
    /// Fully-verified sources are positive info and don't make the report "non-empty".
    pub fn is_empty(&self) -> bool {
        self.unsourced_count == 0 && self.fabricated_count == 0
    }
}

const SPAN_EXCERPT_MAX_CHARS: usize = 160;
/// Cap on reported spans — beyond this the count still grows but we stop
/// storing excerpts (keeps the JSON small on a pathological message).
const MAX_FLAGGED_SPANS: usize = 25;
/// Sentences shorter than this (chars) are ignored — too short to carry a
/// meaningful unsourced claim, and a frequent false-positive source.
const MIN_SENTENCE_CHARS: usize = 25;

/// Claim cues (EN + FR). A sentence containing one of these is treated as a
/// confident technical assertion that ought to carry a source. Curated to keep
/// precision reasonable; tune from telemetry in 0.8.8.
///
/// **0.8.7 patch** : the initial list missed CVE / version / security
/// vocabulary entirely. Real-world disc `0c9c2032` (Symfony CVE diagnostic)
/// surfaced the gap — none of "est vulnérable" / "Corrigé dans" / "Versions
/// affectées" / "CVE-…" triggered. Added the security + version + state
/// patterns below. Full heuristic rewrite (version-number / CVE-id pattern
/// detection) deferred to 0.8.8 with proper telemetry.
const CLAIM_CUES: &[&str] = &[
    // ── English — code / config (original list) ──
    "is located", "is defined", "is implemented", "lives in", "is stored",
    "is configured", "defaults to", "the default is", "returns ", "is handled",
    "the function", "the method", "the endpoint", "the route", "the table",
    "the column", "the config", "the parameter", "the flag", "the option",
    "the api", "is set in", "is declared",
    // ── English — security / CVE / version (0.8.7 patch) ──
    //
    // Kept high-precision on purpose : broader cues like "added in" /
    // "available in" / "the current version" / "ships with" fire on
    // generic prose ("we added in a new flag", "available in the docs")
    // and would spam the pill — explicitly NOT in this list. The cues
    // below are tight verb-frames ("is vulnerable", "fixed in",
    // "patched in") or explicit version-state framings ("the latest
    // version is …") that don't collide with everyday prose.
    "is vulnerable", "is affected by", "patched in", "fixed in",
    "deprecated in version", "affected version", "affected versions",
    "the latest version is", "the installed version is",
    "cve-",
    // ── French — code / config (original list) ──
    "se trouve", "est défini", "est implémenté", "est stocké", "est configuré",
    "par défaut", "renvoie", "la fonction", "la méthode", "la route",
    "la table", "la colonne", "le paramètre", "l'option", "l'endpoint",
    "est déclaré", "est géré",
    // ── French — security / CVE / version (0.8.7 patch) ──
    //
    // Same precision discipline as the English block above. "ajouté dans"
    // / "disponible dans" / "la version actuelle" are intentionally NOT
    // here — they collide with generic prose. The cues below are tight
    // verb-frames ("est vulnérable", "corrigé dans", "patché dans") or
    // explicit version-state framings ("la dernière version est …").
    "est vulnérable", "est affecté par", "patché dans", "corrigé dans",
    "déprécié dans la version", "version affectée", "versions affectées",
    "la dernière version est", "la version installée est",
];

/// Hedges (EN + FR). Their presence suppresses a flag — the agent is already
/// signalling uncertainty, which is exactly the behaviour we want.
const HEDGES: &[&str] = &[
    "i think", "i believe", "i'm not sure", "im not sure", "not sure",
    "maybe", "probably", "might be", "i should check", "unverified",
    "to verify", "let me check", "i'll check", "appears to", "seems to",
    "je pense", "je crois", "peut-être", "peut etre", "probablement",
    "il semble", "à vérifier", "a verifier", "je vérifie", "je verifie",
    "sans certitude", "je ne suis pas sûr", "je ne suis pas sur",
];

/// Does the sentence carry a verifiable anchor (so it's NOT flagged)?
fn has_anchor(lower: &str, original: &str) -> bool {
    if lower.contains("[src:")
        || lower.contains("http://")
        || lower.contains("https://")
        || lower.contains("user-confirmed")
        || lower.contains("user confirmed")
        || lower.contains("confirmé par")
    {
        return true;
    }
    // A backticked span that looks like a path / namespaced identifier / call,
    // or a bare file-path token, counts as self-sourcing.
    contains_code_anchor(original)
}

/// Heuristic: a file path, a `path/like.ext` token, a namespaced identifier,
/// or a `fn()` call ref — any concrete, checkable reference.
fn contains_code_anchor(s: &str) -> bool {
    // File path with a known source extension.
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".toml", ".json", ".sql",
        ".md", ".yml", ".yaml", ".css", ".html", ".sh",
    ];
    let lower = s.to_ascii_lowercase();
    for ext in EXTS {
        // Require the extension to be followed by a word boundary-ish char so
        // "node.js ecosystem" prose doesn't count but "src/foo.js" does — we
        // approximate by also requiring a '/' or alnum-run before the dot.
        if let Some(pos) = lower.find(ext) {
            let before = &lower[..pos];
            if before.ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                && (before.contains('/') || before.contains('\\'))
            {
                return true;
            }
        }
    }
    // Namespaced / path-y identifier inside backticks: `foo::bar`, `a.b.c`,
    // `dir/file`, or a call `foo()`.
    let mut in_tick = false;
    let mut tick_buf = String::new();
    for ch in s.chars() {
        if ch == '`' {
            if in_tick {
                if backtick_looks_like_code(&tick_buf) {
                    return true;
                }
                tick_buf.clear();
            }
            in_tick = !in_tick;
        } else if in_tick {
            tick_buf.push(ch);
        }
    }
    false
}

fn backtick_looks_like_code(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    s.contains("::")
        || s.contains("()")
        || s.contains('/')
        || (s.contains('.') && !s.ends_with('.'))
}

fn contains_any(haystack_lower: &str, needles: &[&str]) -> Option<String> {
    needles
        .iter()
        .find(|n| haystack_lower.contains(*n))
        .map(|n| (*n).to_string())
}

/// Strip fenced ```code blocks``` — agents emit lots of code and linting it
/// would be pure noise. Returns the text with fenced regions removed.
fn strip_fenced_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Split into candidate sentences on `.`, `!`, `?` and newlines. Char-based so
/// it's UTF-8 safe (French accents, emoji). A `.`/`!`/`?` is only a boundary
/// when it's followed by whitespace or end-of-text — so file paths (`retry.rs`),
/// decimals/versions (`v0.8.7`), and domains (`api.example.com`) stay intact and
/// keep their value as anchors. Newlines always split.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let trimmed = cur.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
        cur.clear();
    };
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\n' {
            flush(&mut cur, &mut sentences);
            continue;
        }
        if matches!(ch, '.' | '!' | '?') {
            let is_boundary = match chars.get(i + 1) {
                None => true,
                Some(next) => next.is_whitespace(),
            };
            if is_boundary {
                flush(&mut cur, &mut sentences);
                continue;
            }
        }
        cur.push(ch);
    }
    flush(&mut cur, &mut sentences);
    sentences
}

fn excerpt(s: &str) -> String {
    if s.chars().count() <= SPAN_EXCERPT_MAX_CHARS {
        return s.to_string();
    }
    let mut out: String = s.chars().take(SPAN_EXCERPT_MAX_CHARS).collect();
    out.push('…');
    out
}

/// P2 — flag confident technical assertions left without a verifiable anchor.
///
/// Heuristic, by design lenient (false positives surface as a non-blocking
/// pill). A sentence is flagged when it: is long enough, carries a technical
/// claim cue, has NO hedge, and has NO anchor (file path / URL / `[src:]` /
/// backticked code ref / user-confirmed). Fenced code blocks are excluded.
pub fn lint_assertions(text: &str) -> LintReport {
    if text.trim().is_empty() {
        return LintReport::empty();
    }
    let prose = strip_fenced_code(text);
    let mut spans: Vec<FlaggedSpan> = Vec::new();
    let mut count: u32 = 0;

    for sentence in split_sentences(&prose) {
        if sentence.chars().count() < MIN_SENTENCE_CHARS {
            continue;
        }
        let lower = sentence.to_ascii_lowercase();
        // Hedged → the agent already signalled uncertainty. Good behaviour.
        if contains_any(&lower, HEDGES).is_some() {
            continue;
        }
        // Anchored → self-sourcing.
        if has_anchor(&lower, &sentence) {
            continue;
        }
        if let Some(cue) = contains_any(&lower, CLAIM_CUES) {
            count += 1;
            if spans.len() < MAX_FLAGGED_SPANS {
                spans.push(FlaggedSpan {
                    text: excerpt(&sentence),
                    reason: cue,
                });
            }
        }
    }

    LintReport {
        unsourced_count: count,
        flagged_spans: spans,
        sources: Vec::new(),
        fabricated_count: 0,
    }
}

// ─── Niveau 1 — structured source extraction + mechanical verification ────

/// Hard caps to bound work at message-finalize time.
const MAX_SOURCES_VERIFIED: usize = 50;
/// Skip line-count verification above this file size (existence still checked).
const LINE_COUNT_SIZE_CAP_BYTES: u64 = 2 * 1024 * 1024;

/// Extract every `[src: …]` marker from the text, skipping fenced code blocks
/// (example code shouldn't be verified). Returns the inner content (after
/// `src:`), trimmed. Bracket-balanced so `[src: a[0]]` reads `a[0]`.
pub fn extract_source_markers(text: &str) -> Vec<String> {
    let prose = strip_fenced_code(text);
    let chars: Vec<char> = prose.chars().collect();
    let lower: Vec<char> = prose.to_ascii_lowercase().chars().collect();
    let needle: Vec<char> = "[src:".chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + needle.len() <= chars.len() {
        if lower[i..i + needle.len()] == needle[..] {
            // Read until the matching ']' (balance nested '[').
            let mut depth = 1i32;
            let mut j = i + needle.len();
            let mut buf = String::new();
            while j < chars.len() {
                match chars[j] {
                    '[' => {
                        depth += 1;
                        buf.push('[');
                    }
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        buf.push(']');
                    }
                    c => buf.push(c),
                }
                j += 1;
            }
            out.push(buf.trim().to_string());
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Classify a marker's inner content into a kind + the bare reference, stripping
/// an optional `<type>:` prefix from the v1.1 grammar (`file:`, `url:`, …).
fn classify_source(raw: &str) -> (SourceKind, String) {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();
    // Bare URLs (no type prefix) — never fetched at finalize (SSRF-safe).
    if lower.contains("http://") || lower.contains("https://") {
        return (SourceKind::Url, s.to_string());
    }
    // The `user-confirmed` phrase (the no-colon form).
    if lower.contains("user-confirmed") || lower.contains("user confirmed") {
        return (SourceKind::User, s.to_string());
    }
    // Typed prefixes: keyword optionally followed by `:` or whitespace. The
    // keyword-boundary check means a real file named `user_service.rs`,
    // `api_client.rs`, `commit_log.rs`… is NOT misclassified (it falls through
    // to File). Order: most specific first.
    if let Some(r) = match_type_keyword(s, "training-data") { return (SourceKind::TrainingData, r); }
    if let Some(r) = match_type_keyword(s, "code-comment") { return (SourceKind::CodeComment, r); }
    if let Some(r) = match_type_keyword(s, "inferred") { return (SourceKind::Inferred, r); }
    if let Some(r) = match_type_keyword(s, "hypothesis") { return (SourceKind::Hypothesis, r); }
    if let Some(r) = match_type_keyword(s, "commit") { return (SourceKind::Commit, r); }
    if let Some(r) = match_type_keyword(s, "api") { return (SourceKind::Api, r); }
    if let Some(r) = match_type_keyword(s, "url") { return (SourceKind::Url, r); }
    if let Some(r) = match_type_keyword(s, "user") { return (SourceKind::User, r); }
    if let Some(r) = match_type_keyword(s, "file") { return (SourceKind::File, r); }
    (SourceKind::File, s.to_string())
}

/// If `s` starts (case-insensitively) with `keyword` AT A TOKEN BOUNDARY
/// (followed by end-of-string, `:`, or whitespace), return the trimmed
/// remainder (after an optional `:`). Returns `None` when the keyword is just
/// the prefix of a longer word (e.g. `inferred_thing.rs`), so file paths that
/// happen to start with a type name aren't misclassified. UTF-8 safe.
fn match_type_keyword(s: &str, keyword: &str) -> Option<String> {
    let head = s.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &s[keyword.len()..];
    match rest.chars().next() {
        None => Some(String::new()),
        Some(':') => Some(rest[1..].trim().to_string()),
        Some(c) if c.is_whitespace() => Some(rest.trim().to_string()),
        _ => None,
    }
}

/// Split a file ref into `(path, Option<(start, end)>)`. Accepts `path`,
/// `path:line`, and `path:start-end`. The line spec is only peeled off when the
/// substring after the LAST `:` is purely numeric / a numeric range — so a bare
/// `foo.rs` or an odd `a:b` path isn't mis-parsed.
fn split_path_and_lines(reference: &str) -> (&str, Option<(usize, usize)>) {
    if let Some(idx) = reference.rfind(':') {
        let (path, after) = (&reference[..idx], &reference[idx + 1..]);
        if !after.is_empty() {
            if let Some((s, e)) = parse_line_spec(after) {
                return (path, Some((s, e)));
            }
        }
    }
    (reference, None)
}

fn parse_line_spec(s: &str) -> Option<(usize, usize)> {
    if let Some((a, b)) = s.split_once('-') {
        let start = a.trim().parse::<usize>().ok()?;
        let end = b.trim().parse::<usize>().ok()?;
        if start == 0 || end < start {
            return None;
        }
        Some((start, end))
    } else {
        let n = s.trim().parse::<usize>().ok()?;
        if n == 0 {
            return None;
        }
        Some((n, n))
    }
}

/// Purely lexical path normalisation (resolves `.`/`..` without touching the
/// FS) — used to jail a candidate BEFORE any filesystem access, so a
/// `../../etc/passwd` probe never even stats outside the project root.
fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn count_lines_capped(path: &Path) -> Option<usize> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > LINE_COUNT_SIZE_CAP_BYTES {
        return None; // too big — skip the bounds check, existence is enough
    }
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.lines().count().max(1))
}

/// Verify one `[src: …]` marker against the project root.
///
/// - `file` / `code-comment` → path-jailed existence + line-bounds check
///   (code-comment is verified but flagged LOW trust).
/// - `url` / `api` / `commit` / `user` / `inferred` / `hypothesis` → `Unchecked`
///   (no network at finalize-time — SSRF-safe; soft tiers aren't file-verified).
/// - `training-data` → `Rejected` (model prior knowledge is not a citable source).
pub fn verify_source_marker(raw: &str, project_root: Option<&Path>) -> SourceCheck {
    let (kind, reference) = classify_source(raw);

    let status_detail = match kind {
        SourceKind::Url | SourceKind::Api => {
            (SourceStatus::Unchecked, "URL/API — not network-checked (SSRF-safe)".to_string())
        }
        SourceKind::Commit => (SourceStatus::Unchecked, "commit ref — not verified".to_string()),
        SourceKind::User => {
            (SourceStatus::Unchecked, "user-declared — not machine-verifiable".to_string())
        }
        SourceKind::Inferred => {
            (SourceStatus::Unchecked, "inferred — a derived guess, not a verifiable fact".to_string())
        }
        SourceKind::Hypothesis => {
            (SourceStatus::Unchecked, "hypothesis — unverified, confirm before acting".to_string())
        }
        SourceKind::TrainingData => {
            (SourceStatus::Rejected, "training-data is not a citable source — refused".to_string())
        }
        SourceKind::Other => {
            (SourceStatus::Unchecked, "unknown source type — not verified".to_string())
        }
        // A code comment still gets existence-verified, but is flagged LOW trust.
        SourceKind::CodeComment => {
            let (st, detail) = verify_file_ref(&reference, project_root);
            (st, format!("code comment (not authoritative — verify): {}", detail))
        }
        SourceKind::File => verify_file_ref(&reference, project_root),
    };

    SourceCheck {
        raw: raw.trim().to_string(),
        kind,
        status: status_detail.0,
        detail: status_detail.1,
    }
}

fn verify_file_ref(reference: &str, project_root: Option<&Path>) -> (SourceStatus, String) {
    let reference = reference.trim();
    if reference.is_empty() {
        return (SourceStatus::EmptyRef, "empty source reference".into());
    }
    let root = match project_root {
        Some(r) => r,
        None => {
            return (
                SourceStatus::Unchecked,
                "no project root — can't resolve file path".into(),
            )
        }
    };

    let (path_str, line_spec) = split_path_and_lines(reference);
    let path = Path::new(path_str);

    // 0.8.7 semantic (user decision 2026-05-28) : an absolute path in a
    // citation is checked for **existence**, period. No project-root jail
    // and no linked_repos plumbing — the agent emits a fully-qualified
    // path, the file either exists on the filesystem or it doesn't. This
    // is what eliminates the false-positives on cross-repo references
    // (linked_repos, monorepos, sister sites) that the previous
    // jail-everything approach reported as "not found / outside project".
    //
    // For Docker runs we still need to translate `/home/<user>/…` to the
    // container-visible `/host-home/…` so the existence check hits the
    // mounted host tree. `scanner::resolve_host_path` is the same
    // translator the runner uses to resolve project paths.
    //
    // Relative paths keep their lexical jail under `root` so a `../etc/`
    // smuggled inside a relative citation still resolves OutsideProject.
    let (candidate, check_symlink_escape): (PathBuf, bool) = if path.is_absolute() {
        (crate::core::scanner::resolve_host_path(path_str), false)
    } else {
        let joined = root.join(path);
        let norm_joined = normalize_lexical(&joined);
        let norm_root = normalize_lexical(root);
        if !norm_joined.starts_with(&norm_root) {
            return (
                SourceStatus::OutsideProject,
                "relative path escapes the project root via ../".into(),
            );
        }
        // Symlink escape guard — only for the RELATIVE case. The lexical
        // jail above is pure string matching ; a `subdir/leak.txt` where
        // `subdir/` is a symlink to `/etc/` passes the lexical check but
        // resolves to a path outside the project on disk. Re-check after
        // canonicalisation. (Absolute paths are explicitly existence-only
        // per the 2026-05-28 design decision — they're trusted to point
        // wherever the agent saw the file.)
        (joined, true)
    };

    if !candidate.exists() {
        return (SourceStatus::NotFound, format!("file not found: {}", path_str));
    }

    if check_symlink_escape {
        if let (Ok(canon), Ok(canon_root)) = (candidate.canonicalize(), root.canonicalize()) {
            if !canon.starts_with(&canon_root) {
                return (
                    SourceStatus::OutsideProject,
                    "resolves (via symlink) outside the project root".into(),
                );
            }
        }
    }

    match line_spec {
        None => (SourceStatus::Verified, "file exists".into()),
        Some((start, end)) => match count_lines_capped(&candidate) {
            None => (
                SourceStatus::Verified,
                "file exists (too large to bounds-check lines)".into(),
            ),
            Some(total) => {
                if end <= total {
                    (SourceStatus::Verified, format!("lines {}-{} within {} lines", start, end, total))
                } else {
                    (
                        SourceStatus::OutOfBounds,
                        format!("line {} beyond file length {}", end, total),
                    )
                }
            }
        },
    }
}

/// Full P2 analysis: niveau 0 heuristic + niveau 1 mechanical source
/// verification. `project_root` is the disc's effective working tree (project
/// path, or worktree path in test/isolated mode); `None` for project-less discs
/// (file refs become `Unchecked`).
pub fn analyze(text: &str, project_root: Option<&Path>) -> LintReport {
    let mut report = lint_assertions(text);
    let mut fabricated = 0u32;
    let mut sources = Vec::new();
    for raw in extract_source_markers(text).into_iter().take(MAX_SOURCES_VERIFIED) {
        let check = verify_source_marker(&raw, project_root);
        if check.status.is_fabricated() {
            fabricated += 1;
        }
        sources.push(check);
    }
    report.sources = sources;
    report.fabricated_count = fabricated;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Mode ──────────────────────────────────────────────────────────

    #[test]
    fn mode_parse_is_lenient_and_defaults_to_warn() {
        assert_eq!(AntiHallucMode::from_str_lenient("off"), AntiHallucMode::Off);
        assert_eq!(AntiHallucMode::from_str_lenient("WARN"), AntiHallucMode::Warn);
        assert_eq!(AntiHallucMode::from_str_lenient(" Enforce "), AntiHallucMode::Enforce);
        // Unknown / empty → warn (rollout default).
        assert_eq!(AntiHallucMode::from_str_lenient("bogus"), AntiHallucMode::Warn);
        assert_eq!(AntiHallucMode::from_str_lenient(""), AntiHallucMode::Warn);
    }

    #[test]
    fn mode_round_trips_and_active_flag() {
        for m in [AntiHallucMode::Off, AntiHallucMode::Warn, AntiHallucMode::Enforce] {
            assert_eq!(AntiHallucMode::from_str_lenient(m.as_str()), m);
        }
        assert!(!AntiHallucMode::Off.is_active());
        assert!(AntiHallucMode::Warn.is_active());
        assert!(AntiHallucMode::Enforce.is_active());
    }

    #[test]
    fn is_valid_mode_accepts_only_three() {
        for ok in ["off", "warn", "enforce", "OFF", " Warn "] {
            assert!(is_valid_mode(ok), "{ok} should be valid");
        }
        for bad in ["", "on", "strict", "warning"] {
            assert!(!is_valid_mode(bad), "{bad} should be invalid");
        }
    }

    #[test]
    fn preamble_gating_follows_global_mode() {
        set_mode("off");
        assert!(preamble_if_active().is_none());
        set_mode("warn");
        assert!(preamble_if_active().is_some());
        set_mode("enforce");
        assert!(preamble_if_active().is_some());
        // Restore the process default so this test doesn't leak state into
        // other tests in the same binary.
        set_mode(DEFAULT_MODE_STR);
    }

    #[test]
    fn preamble_contract_pins_pointer_and_citation_grammar() {
        // 0.8.7 — PREAMBLE was shortened to a POINTER toward
        // `docs/AGENTS.md` § anti-hallucination (the canonical source).
        // The pointer MUST still carry the citation grammar inline so
        // the agent has the format reference even before reading the
        // doc (token-budget safety net). Pin both : the pointer phrase
        // toward AGENTS.md + the structured `[src: …]` grammar.
        assert!(
            PREAMBLE.contains("`docs/AGENTS.md`") || PREAMBLE.contains("docs/AGENTS.md"),
            "PREAMBLE must point to `docs/AGENTS.md` (the canonical source)",
        );
        assert!(
            PREAMBLE.contains("anti-hallucination"),
            "PREAMBLE must mention the anti-hallucination section name",
        );
        // The structured citation grammar — `[src: file:]` is the form
        // mechanically verified by `verify_source_marker` ; the others
        // ship the format reference inline so an agent that hasn't
        // loaded AGENTS.md yet still knows how to cite.
        assert!(PREAMBLE.contains("[src: file:"), "[src: file:] grammar missing");
        assert!(PREAMBLE.contains("[src: url:"), "[src: url:] grammar missing");
        // Honesty discipline — fabrication = path/line that doesn't
        // resolve or escapes the project root.
        assert!(
            PREAMBLE.contains("rejected as fabricated")
                || PREAMBLE.contains("escape") && PREAMBLE.contains("rejected"),
            "PREAMBLE must warn that bad citations are rejected",
        );
        // Size sanity — PREAMBLE is the fallback pointer, not the
        // full doctrine. If it grows past ~250 words it means someone
        // re-inlined the cascade and forgot the redesign.
        let word_count = PREAMBLE.split_whitespace().count();
        assert!(
            word_count < 200,
            "PREAMBLE ballooned to {word_count} words — it must stay a short pointer (the cascade lives in docs/AGENTS.md § anti-hallu)",
        );
    }

    // ── Linter: flags ─────────────────────────────────────────────────

    #[test]
    fn flags_unsourced_technical_claim() {
        let r = lint_assertions(
            "The retry logic is implemented with an exponential backoff loop.",
        );
        assert_eq!(r.unsourced_count, 1);
        assert_eq!(r.flagged_spans.len(), 1);
    }

    #[test]
    fn flags_french_unsourced_claim() {
        let r = lint_assertions(
            "La fonction de cache se trouve dans le coeur du serveur résilient.",
        );
        assert_eq!(r.unsourced_count, 1, "report: {:?}", r);
    }

    #[test]
    fn counts_multiple_claims() {
        let txt = "The auth token is stored in the session table. \
                   The default is a thirty minute expiry window for everyone.";
        let r = lint_assertions(txt);
        assert_eq!(r.unsourced_count, 2);
    }

    // ── Linter: suppressions ──────────────────────────────────────────

    #[test]
    fn does_not_flag_sentence_with_file_path_anchor() {
        let r = lint_assertions(
            "The retry logic is implemented in backend/src/core/retry.rs for all callers.",
        );
        assert_eq!(r.unsourced_count, 0, "file path is a self-source: {:?}", r);
    }

    #[test]
    fn does_not_flag_sentence_with_backticked_code_ref() {
        let r = lint_assertions(
            "The default is resolved by `config::default_model_tier()` at startup.",
        );
        assert_eq!(r.unsourced_count, 0, "backtick code ref anchors it: {:?}", r);
    }

    #[test]
    fn does_not_flag_sentence_with_url() {
        let r = lint_assertions(
            "The endpoint returns paginated JSON per https://api.example.com/docs spec.",
        );
        assert_eq!(r.unsourced_count, 0);
    }

    #[test]
    fn does_not_flag_hedged_claim() {
        let r = lint_assertions(
            "I think the function probably returns the cached value, but let me check.",
        );
        assert_eq!(r.unsourced_count, 0, "hedge suppresses the flag: {:?}", r);
    }

    #[test]
    fn does_not_flag_src_annotated_claim() {
        let r = lint_assertions(
            "The default is a 30 minute window [src: backend session config].",
        );
        assert_eq!(r.unsourced_count, 0);
    }

    // ── 0.8.7 patch — CVE / version / security vocabulary ─────────────
    // Real-world disc 0c9c2032 (Symfony CVE diagnostic) exposed the
    // original heuristic's blind spot : none of the agent's claims
    // matched a cue, so `lint_report` stayed `None` and the pill never
    // surfaced. These pin the expanded `CLAIM_CUES` so that regression
    // can't happen silently.

    #[test]
    fn flags_french_security_vulnerability_claim() {
        let r = lint_assertions("La version 7.3.6 est vulnérable à une faille de PATH_INFO.");
        assert!(r.unsourced_count >= 1, "expected vulnérable to flag: {:?}", r);
    }

    #[test]
    fn flags_french_affected_versions_claim() {
        let r = lint_assertions("Les versions affectées vont de 6.0 à 7.3.6 d'après l'avis.");
        assert!(r.unsourced_count >= 1, "expected versions affectées to flag: {:?}", r);
    }

    #[test]
    fn flags_french_fixed_in_claim() {
        let r = lint_assertions("La faille est corrigé dans la release 7.3.7 publiée hier.");
        assert!(r.unsourced_count >= 1, "expected corrigé dans to flag: {:?}", r);
    }

    #[test]
    fn flags_english_cve_id_claim() {
        let r = lint_assertions("CVE-2025-64500 is a high-severity issue in Symfony's PATH_INFO handling.");
        assert!(r.unsourced_count >= 1, "expected cve- to flag: {:?}", r);
    }

    #[test]
    fn flags_english_fixed_in_claim() {
        let r = lint_assertions("The bug is fixed in version 7.3.7 according to the upstream changelog.");
        assert!(r.unsourced_count >= 1, "expected fixed in to flag: {:?}", r);
    }

    #[test]
    fn cve_claim_with_source_marker_is_not_flagged() {
        let r = lint_assertions(
            "CVE-2025-64500 affects 7.3.6 [src: https://symfony.com/blog/cve-2025-64500].",
        );
        assert_eq!(r.unsourced_count, 0, "anchored CVE claim must not flag: {:?}", r);
    }

    #[test]
    fn hedged_cve_claim_is_not_flagged() {
        let r = lint_assertions("I think CVE-2025-64500 might be the relevant one, let me check.");
        assert_eq!(r.unsourced_count, 0, "hedged CVE claim must not flag: {:?}", r);
    }

    // ── Anti-FP regression : the broader cues we explicitly REJECTED
    // must not trigger on everyday prose. These pin the precision
    // discipline of the 0.8.7 patch — if a future expansion re-adds
    // any of these, the corresponding test fails and forces the
    // decision back to a human.

    #[test]
    fn does_not_flag_generic_added_in_phrase() {
        // "added in" is intentionally NOT a cue — too broad, fires on
        // build-log / changelog prose that's already self-anchoring.
        let r = lint_assertions("We added in a new dropdown for the user picker last sprint.");
        assert_eq!(r.unsourced_count, 0, "generic 'added in' must not flag: {:?}", r);
    }

    #[test]
    fn does_not_flag_generic_available_in_phrase() {
        let r = lint_assertions("That feature is available in the documentation we just shipped.");
        assert_eq!(r.unsourced_count, 0, "generic 'available in' must not flag: {:?}", r);
    }

    #[test]
    fn does_not_flag_generic_current_version_phrase() {
        // "the current version" alone is too generic — keep the FP risk
        // off the table unless it's framed as "the current version is …".
        let r = lint_assertions("Reviewing the current version of the layout before we ship.");
        assert_eq!(r.unsourced_count, 0, "generic 'current version' must not flag: {:?}", r);
    }

    #[test]
    fn does_not_flag_generic_disponible_dans() {
        let r = lint_assertions("Cette fonctionnalité est disponible dans la doc mise à jour hier.");
        assert_eq!(r.unsourced_count, 0, "generic 'disponible dans' must not flag: {:?}", r);
    }

    #[test]
    fn does_not_flag_generic_ajoute_dans() {
        let r = lint_assertions("On a ajouté dans la sidebar un raccourci vers les paramètres.");
        assert_eq!(r.unsourced_count, 0, "generic 'ajouté dans' must not flag: {:?}", r);
    }

    // ── Anti-FP for the 3 historical categories (0.8.7 R2 — note from
    // anti-hallu expert integrated by tech-lead) :
    //   (a) citations of doc inside fenced code [already covered above]
    //   (b) proper nouns with FR/ES apostrophes
    //   (c) bare semantic versions in non-claim prose

    #[test]
    fn does_not_flag_proper_noun_with_apostrophe_fr() {
        // Apostrophes in FR proper nouns ("L'équipe", "d'Élodie") used to
        // trip the heuristic when the surrounding cue-like tokens looked
        // technical. Real-world miss : "L'équipe d'Élodie a configuré le
        // déploiement chez Vercel." has zero verifiable claim but the
        // apostrophe-heavy phrasing must NOT trip the linter.
        let r = lint_assertions("L'équipe d'Élodie a configuré le déploiement chez Vercel.");
        assert_eq!(
            r.unsourced_count, 0,
            "FR proper noun with apostrophes must not flag: {:?}",
            r,
        );
    }

    #[test]
    fn does_not_flag_proper_noun_with_apostrophe_es() {
        // Spanish has fewer apostrophe-y constructs but loanwords + names
        // (Ortiz's, O'Brien, l'Hospital) appear in code comments and
        // commit messages. None of them should trip the linter.
        let r = lint_assertions("Ortiz's PR landed yesterday after O'Brien reviewed it.");
        assert_eq!(
            r.unsourced_count, 0,
            "ES/EN apostrophe proper noun must not flag: {:?}",
            r,
        );
    }

    #[test]
    fn does_not_flag_bare_semver_in_neutral_prose() {
        // Bare semantic versions ("Version 1.2.3 sort demain.") appear
        // in release-notes-style prose without being a verifiable claim
        // about THIS project. Without a claim cue ("is fixed in",
        // "affected versions"), bare semver must NOT trip the linter —
        // otherwise every changelog blurb generates noise.
        let r = lint_assertions("Version 1.2.3 sort demain selon le calendrier prévu.");
        assert_eq!(
            r.unsourced_count, 0,
            "bare semver in neutral prose must not flag: {:?}",
            r,
        );
    }

    #[test]
    fn corpus_false_positive_rate_under_5_percent() {
        // 0.8.7 redesign R2 P1-B (architecte) — quantified FP gate.
        // Loads `backend/tests/fixtures/anti_halluc_corpus.jsonl` (50
        // messages, each labelled with its `expected_unsourced` count).
        // A FP = a message where the linter flags MORE assertions than
        // expected. We assert the FP rate stays under 5% — Wilson 95%
        // confidence interval at n=50 → [0.014, 0.165], puissance
        // suffisante pour détecter un régresseur 5%→15%+.
        //
        // ⚠ When CLAIM_CUES expand, this test catches an over-broad
        // change before it lands. If a real positive needs to be added
        // (i.e. a NEW kind of legitimate claim), update the corpus
        // entry's `expected_unsourced` field accordingly.
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/anti_halluc_corpus.jsonl");
        let content = std::fs::read_to_string(&fixture_path)
            .expect("fixture must exist");

        let mut total = 0usize;
        let mut false_positives = 0usize;
        let mut fp_examples: Vec<String> = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("bad JSONL line: {line}\n{e}"));
            let text = entry["text"].as_str().expect("text field");
            let expected = entry["expected_unsourced"].as_u64().expect("expected_unsourced field") as u32;
            let category = entry["category"].as_str().unwrap_or("?");

            let report = lint_assertions(text);
            total += 1;
            if report.unsourced_count > expected {
                false_positives += 1;
                if fp_examples.len() < 5 {
                    fp_examples.push(format!(
                        "[{category}] expected={expected} got={} text=\"{}\"",
                        report.unsourced_count,
                        text.chars().take(80).collect::<String>(),
                    ));
                }
            }
        }

        assert!(total >= 50, "corpus must have ≥ 50 entries, got {total}");
        let fp_rate = false_positives as f64 / total as f64;
        assert!(
            fp_rate <= 0.05,
            "FP rate {:.1}% exceeds 5% gate ({fp}/{total}). Examples:\n  {}",
            fp_rate * 100.0,
            fp_examples.join("\n  "),
            fp = false_positives,
            total = total,
        );
    }

    #[test]
    fn ignores_fenced_code_blocks() {
        let txt = "Here is the impl:\n```rust\nlet the_endpoint = returns_default();\n```\nDone.";
        let r = lint_assertions(txt);
        // The claim-cue-looking tokens are inside the fence → ignored.
        assert_eq!(r.unsourced_count, 0, "fenced code must not be linted: {:?}", r);
    }

    #[test]
    fn ignores_short_and_non_technical_prose() {
        let r = lint_assertions("Sounds good. Thanks! I'll get started now and report back soon.");
        assert_eq!(r.unsourced_count, 0);
    }

    #[test]
    fn empty_input_is_empty_report() {
        assert!(lint_assertions("").is_empty());
        assert!(lint_assertions("   \n  ").is_empty());
    }

    // ── Linter: robustness ────────────────────────────────────────────

    #[test]
    fn excerpt_is_char_boundary_safe_with_multibyte() {
        // 300 accented chars in one flagged sentence → excerpt must not panic
        // and must cap at SPAN_EXCERPT_MAX_CHARS + 1 (ellipsis).
        let long = format!("The function {} se trouve ici", "é".repeat(300));
        let r = lint_assertions(&long);
        assert_eq!(r.unsourced_count, 1);
        assert!(r.flagged_spans[0].text.chars().count() <= SPAN_EXCERPT_MAX_CHARS + 1);
    }

    #[test]
    fn emoji_in_claim_does_not_panic() {
        let r = lint_assertions("The route is handled by the gateway 🚀 reliably for users.");
        // Whether flagged or not, the point is no panic + valid report.
        assert!(r.unsourced_count <= 1);
    }

    #[test]
    fn span_count_is_capped_but_count_is_accurate() {
        // Build 30 distinct unsourced claims; count tracks all, spans cap.
        let mut txt = String::new();
        for i in 0..30 {
            txt.push_str(&format!(
                "The parameter number {i} is configured for the whole fleet here. "
            ));
        }
        let r = lint_assertions(&txt);
        assert_eq!(r.unsourced_count, 30);
        assert_eq!(r.flagged_spans.len(), MAX_FLAGGED_SPANS);
    }

    // ── Niveau 1 — extraction ─────────────────────────────────────────

    #[test]
    fn extract_multiple_markers() {
        let txt = "A [src: foo.rs:1] and B [src: url: https://x.com] end.";
        let m = extract_source_markers(txt);
        assert_eq!(m, vec!["foo.rs:1", "url: https://x.com"]);
    }

    #[test]
    fn extract_skips_markers_inside_fences() {
        let txt = "Real [src: a.rs:1].\n```\nfake [src: b.rs:9]\n```\nDone.";
        let m = extract_source_markers(txt);
        assert_eq!(m, vec!["a.rs:1"], "fenced marker must be ignored");
    }

    #[test]
    fn extract_balances_nested_brackets() {
        let m = extract_source_markers("see [src: arr[0].field] here");
        assert_eq!(m, vec!["arr[0].field"]);
    }

    #[test]
    fn extract_empty_when_none() {
        assert!(extract_source_markers("no markers here at all").is_empty());
    }

    // ── Niveau 1 — line-spec parsing ──────────────────────────────────

    #[test]
    fn split_path_and_lines_variants() {
        assert_eq!(split_path_and_lines("foo.rs"), ("foo.rs", None));
        assert_eq!(split_path_and_lines("foo.rs:42"), ("foo.rs", Some((42, 42))));
        assert_eq!(split_path_and_lines("a/b/foo.rs:10-20"), ("a/b/foo.rs", Some((10, 20))));
        // A non-numeric tail is part of the path, not a line spec.
        assert_eq!(split_path_and_lines("weird:name"), ("weird:name", None));
        // Reversed range is rejected → treated as no line spec.
        assert_eq!(split_path_and_lines("foo.rs:20-10"), ("foo.rs:20-10", None));
    }

    // ── Niveau 1 — classification ─────────────────────────────────────

    #[test]
    fn classify_detects_kinds() {
        assert_eq!(classify_source("https://x.com").0, SourceKind::Url);
        assert_eq!(classify_source("url: https://x.com").0, SourceKind::Url);
        assert_eq!(classify_source("user-confirmed 2026-05-25").0, SourceKind::User);
        assert_eq!(classify_source("commit: abc123").0, SourceKind::Commit);
        assert_eq!(classify_source("file: src/a.rs:1").0, SourceKind::File);
        // Bare path defaults to File, prefix stripped.
        let (k, r) = classify_source("file: src/a.rs:1");
        assert_eq!(k, SourceKind::File);
        assert_eq!(r, "src/a.rs:1");
    }

    #[test]
    fn classify_full_provenance_gradient() {
        assert_eq!(classify_source("training-data: anything").0, SourceKind::TrainingData);
        assert_eq!(classify_source("training-data").0, SourceKind::TrainingData);
        assert_eq!(classify_source("inferred: backend/src/api/").0, SourceKind::Inferred);
        assert_eq!(classify_source("hypothesis").0, SourceKind::Hypothesis);
        assert_eq!(classify_source("code-comment: GptLoader.ts:36").0, SourceKind::CodeComment);
        assert_eq!(classify_source("api: log#9").0, SourceKind::Api);
    }

    #[test]
    fn classify_does_not_misclassify_files_starting_with_type_names() {
        // Real files whose names begin with a type keyword stay File (the
        // token-boundary check) — regression guard for the `user_service.rs` bug.
        for path in ["user_service.rs:10", "api_client.rs:1", "commit_log.rs", "filer.rs"] {
            assert_eq!(classify_source(path).0, SourceKind::File, "{path}");
        }
    }

    #[test]
    fn training_data_is_rejected_not_verified() {
        // The spec's headline promise: training-data is refused, never file-verified.
        let c = verify_source_marker("training-data: I recall that…", Some(Path::new("/tmp")));
        assert_eq!(c.kind, SourceKind::TrainingData);
        assert_eq!(c.status, SourceStatus::Rejected);
        assert!(c.status.is_fabricated(), "rejected must count toward fabricated_count");
    }

    #[test]
    fn inferred_and_hypothesis_are_unchecked_not_file_verified() {
        let root = temp_project();
        // Even with a real file ref, inferred is soft → never file-verified.
        let inf = verify_source_marker("inferred: src/foo.rs", Some(&root));
        assert_eq!(inf.kind, SourceKind::Inferred);
        assert_eq!(inf.status, SourceStatus::Unchecked);
        assert!(!inf.status.is_fabricated());
        let hyp = verify_source_marker("hypothesis", Some(&root));
        assert_eq!(hyp.status, SourceStatus::Unchecked);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn code_comment_is_existence_verified_but_low_trust() {
        let root = temp_project();
        let ok = verify_source_marker("code-comment: src/foo.rs:2", Some(&root));
        assert_eq!(ok.kind, SourceKind::CodeComment);
        assert_eq!(ok.status, SourceStatus::Verified);
        assert!(ok.detail.contains("not authoritative"), "detail: {}", ok.detail);
        // A code-comment citing a missing file is still flagged fabricated.
        let bad = verify_source_marker("code-comment: src/ghost.rs:1", Some(&root));
        assert_eq!(bad.status, SourceStatus::NotFound);
        assert!(bad.status.is_fabricated());
        std::fs::remove_dir_all(&root).ok();
    }

    // ── Niveau 1 — mechanical verification (real temp project) ────────

    fn temp_project() -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("kronn_antihalluc_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(d.join("src")).unwrap();
        // A 5-line file at src/foo.rs.
        std::fs::write(d.join("src/foo.rs"), "a\nb\nc\nd\ne\n").unwrap();
        d
    }

    #[test]
    fn verify_existing_file_no_line() {
        let root = temp_project();
        let c = verify_source_marker("src/foo.rs", Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{:?}", c);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_line_in_and_out_of_bounds() {
        let root = temp_project();
        assert_eq!(verify_source_marker("src/foo.rs:3", Some(&root)).status, SourceStatus::Verified);
        assert_eq!(verify_source_marker("src/foo.rs:5", Some(&root)).status, SourceStatus::Verified);
        assert_eq!(verify_source_marker("src/foo.rs:6", Some(&root)).status, SourceStatus::OutOfBounds);
        // Range
        assert_eq!(verify_source_marker("src/foo.rs:2-4", Some(&root)).status, SourceStatus::Verified);
        assert_eq!(verify_source_marker("src/foo.rs:4-9", Some(&root)).status, SourceStatus::OutOfBounds);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_missing_file_is_not_found() {
        let root = temp_project();
        let c = verify_source_marker("src/nope.rs:1", Some(&root));
        assert_eq!(c.status, SourceStatus::NotFound, "{:?}", c);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_relative_path_traversal_is_jailed() {
        // Relative paths still go through the lexical jail under `root`,
        // so a `../` smuggled inside a relative citation is OutsideProject.
        let root = temp_project();
        let rel = verify_source_marker("../../../../../../etc/passwd:1", Some(&root));
        assert_eq!(rel.status, SourceStatus::OutsideProject, "{:?}", rel);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_absolute_path_is_existence_only_no_jail() {
        // 0.8.7 policy (decided 2026-05-28) : an absolute citation is
        // checked for existence on the host filesystem, period. No
        // project-root jail. This is what unblocks the linked_repos /
        // monorepo / sister-site references the agent legitimately
        // emits (e.g. a `front_africanews` file cited from a
        // `front_euronews` discussion). `/etc/passwd` exists on every
        // Linux, so it must verify under the new rule — this codifies
        // the policy change explicitly.
        let root = temp_project();
        let c = verify_source_marker("/etc/passwd", Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{:?}", c);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_absolute_path_to_sibling_dir_is_verified() {
        // The actual bug-fix case : project root in /tmp/foo, citation
        // points at /tmp/foo-sibling/file — outside `root` lexically,
        // but exists on disk. Pre-fix this returned NotFound /
        // OutsideProject (false positive) ; under the new rule it
        // verifies.
        let root = temp_project();
        let sibling = std::env::temp_dir().join(format!(
            "kronn-lint-test-sibling-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(sibling.join("src")).unwrap();
        std::fs::write(sibling.join("src/data.yml"), "hello: world\n").unwrap();
        let abs_ref = format!("{}", sibling.join("src/data.yml").display());

        let c = verify_source_marker(&abs_ref, Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{:?}", c);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&sibling).ok();
    }

    #[test]
    fn verify_absolute_path_that_does_not_exist_is_not_found() {
        // The honest failure mode under the new rule : an absolute path
        // that doesn't exist anywhere on the host returns NotFound (not
        // OutsideProject), and the detail surfaces the path as-emitted.
        let root = temp_project();
        let missing = "/this/path/should/not/exist/anywhere-7a3f.txt:1";
        let c = verify_source_marker(missing, Some(&root));
        assert_eq!(c.status, SourceStatus::NotFound, "{:?}", c);
        std::fs::remove_dir_all(&root).ok();
    }

    // P0-5 of the QA roadmap — symlink escape via a RELATIVE path. The
    // lexical jail catches `../` traversal in `verify_relative_path_…`
    // above ; it does NOT catch a symlink whose components look innocent
    // but whose canonical resolution points outside the project. After
    // the 2026-05-28 absolute-path-existence-only policy was applied,
    // the canonicalize re-check was preserved only for the relative
    // branch (absolute citations are trusted by design). This test pins
    // that the relative-symlink-escape attack stays blocked.
    #[cfg(unix)]
    #[test]
    fn verify_relative_symlink_escape_is_outside_project() {
        use std::os::unix::fs::symlink;

        let root = temp_project();
        // Create a directory OUTSIDE the project root that holds a secret.
        let outside = std::env::temp_dir().join(format!(
            "kronn_antihalluc_outside_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "leaked\n").unwrap();
        // Plant a symlink INSIDE the project root that points at the
        // outside dir. A naïve lexical check on `link/secret.txt` would
        // resolve to root/link/secret.txt → starts_with(root) → pass.
        symlink(&outside, root.join("link")).unwrap();

        let c = verify_source_marker("link/secret.txt", Some(&root));
        assert_eq!(
            c.status,
            SourceStatus::OutsideProject,
            "symlink escape must be caught by the canonicalize re-check; got {c:?}"
        );
        assert!(
            c.detail.to_lowercase().contains("symlink"),
            "detail should hint at symlink: {}",
            c.detail
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn verify_relative_symlink_inside_project_stays_verified() {
        // Companion to the escape test — a symlink INSIDE the project
        // (pointing at another file inside the same root) must NOT be
        // rejected. Otherwise the canonicalize guard would over-fire
        // on legitimate symlinked vendored deps / submodules.
        use std::os::unix::fs::symlink;

        let root = temp_project();
        // root/src/foo.rs already exists ; alias it as root/alias.rs.
        symlink(root.join("src/foo.rs"), root.join("alias.rs")).unwrap();

        let c = verify_source_marker("alias.rs", Some(&root));
        assert_eq!(
            c.status,
            SourceStatus::Verified,
            "intra-project symlinks must remain Verified: {c:?}"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_empty_ref_and_no_root() {
        let root = temp_project();
        assert_eq!(verify_source_marker("file:", Some(&root)).status, SourceStatus::EmptyRef);
        // No project root → can't resolve a file path → Unchecked, not fabricated.
        assert_eq!(verify_source_marker("src/foo.rs:1", None).status, SourceStatus::Unchecked);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_url_and_user_are_unchecked_not_fabricated() {
        let url = verify_source_marker("url: https://example.com/doc", Some(Path::new("/tmp")));
        assert_eq!(url.kind, SourceKind::Url);
        assert_eq!(url.status, SourceStatus::Unchecked);
        assert!(!url.status.is_fabricated());
        let user = verify_source_marker("user-confirmed 2026-05-25", Some(Path::new("/tmp")));
        assert_eq!(user.kind, SourceKind::User);
        assert!(!user.status.is_fabricated());
    }

    #[test]
    fn analyze_combines_heuristic_and_mechanical() {
        let root = temp_project();
        let txt = "The retry logic is implemented [src: src/foo.rs:2]. \
                   The cache is implemented [src: src/ghost.rs:1]. \
                   The worker pool is implemented with great care everywhere.";
        let r = analyze(txt, Some(&root));
        // ghost.rs doesn't exist → 1 fabricated; foo.rs:2 verified.
        assert_eq!(r.fabricated_count, 1, "sources: {:?}", r.sources);
        assert_eq!(r.sources.len(), 2);
        // The third sentence has a cue + no anchor → niveau 0 flags it; the
        // first two are anchored by their [src:] markers so they don't.
        assert_eq!(r.unsourced_count, 1, "spans: {:?}", r.flagged_spans);
        assert!(!r.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
