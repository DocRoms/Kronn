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
//!
//! ## Known structural limits (honest record — 2026-05-30 forensic re-pass)
//! P2 verifies file/line ANCHORS: a path resolves, a line is in bounds. It does
//! NOT read a file's content. A deep 4-conversation forensic re-pass confirmed
//! these are out of reach BY CONSTRUCTION (flagging them would be guesswork /
//! false positives), not bugs:
//!   - **Semantic content** — a claim ABOUT what a file contains. The one real
//!     hallucination found ("the CSS `nth-of-type(even)` already in place…",
//!     when no such rule exists) is invisible: it carries no anchor and we never
//!     diff prose against file bodies. A future niveau-2 "content grep" would be
//!     the only catch.
//!   - **Verbatim quotes** — an exact-looking quote from a doc isn't checked
//!     against the doc's real text.
//!   - **i18n keys** — `.xlf` resolves as a FILE; a translation *key* inside it
//!     is not looked up (would need XLIFF parsing). Don't oversell `.xlf`.
//!   - **Absence claims** — "X is mentioned nowhere" can't be proven from an
//!     anchor (proving a negative needs a search, not a bounds check).
//!   - **Bare basenames** — `README.md` with no `/` or `:line` stays unresolved
//!     on purpose (basename-only resolution is ambiguous → precision loss).
//!   - **Approximate `~N` line numbers** detached from their path in prose.

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

// ─── P3 (0.8.8 PR-B) — enforce-mode disc policy helpers ──────────────────
//
// Pure decision functions so the runner / streaming wiring is unit-testable
// without spawning a live agent (the call sites supply the FS check + mode).

/// Whether the enforce gate should auto-attach the `kronn-doc-author` skill to
/// an agent run: only in enforce, only when the project carries a
/// `docs/AGENTS.md`, and only if the skill isn't already attached (idempotent).
pub fn should_auto_attach_doc_author(
    mode: AntiHallucMode,
    skill_ids: &[String],
    project_has_agents_md: bool,
) -> bool {
    mode == AntiHallucMode::Enforce
        && project_has_agents_md
        && !skill_ids.iter().any(|s| s == "kronn-doc-author")
}

/// Whether a finalized agent message must get the enforce P3 refusal note: only
/// in enforce, and only when ≥1 formal `[src:]` citation is high-confidence
/// fabricated.
pub fn enforce_refusal_needed(mode: AntiHallucMode, fabricated_count: u32) -> bool {
    mode == AntiHallucMode::Enforce && fabricated_count > 0
}

/// The System message surfaced when a disc reply is refused under enforce.
/// Non-destructive: the agent message stays; this note tells the human to get
/// it corrected before relying on it (no auto-retry — the user arbitrates).
pub fn enforce_refusal_message(fabricated_count: u32) -> String {
    format!(
        "⛔ Réponse refusée (anti-hallucination · enforce) : {fabricated_count} citation(s) `[src: …]` \
fabriquée(s) — fichier ou ligne introuvable / hors projet. La réponse est conservée mais NON validée : \
demandez à l'agent de corriger ou retirer ces citations avant de vous en servir."
    )
}

// ─── P1 — directive ──────────────────────────────────────────────────────

/// The sourcing-discipline directive injected into agent prompts.
///
/// Plain-language in 0.8.7 on purpose: it must work before the formal
/// provenance spec exists (Phase 2). Once the spec ships, tighten the wording
/// to point at the `[src: <type>: <ref>]` grammar + the public spec URL.
pub const PREAMBLE: &str = "\
=== ANTI-HALLUCINATION (pointer) ===

Never state a non-trivial technical fact (file paths, function / API / config \
names, versions, behaviour) you have not verified. Cascade: read the code → \
read docs/ → official external doc → ask the user. \"I don't know yet, let me \
check\" beats a guess.

Anchor every such fact so it stays checkable. In normal replies a backticked \
path or `path:line` (e.g. `backend/src/lib.rs:440`) or a URL counts as a valid \
source — Kronn auto-verifies these. In curated `docs/AGENTS.md` sections use \
the formal grammar `[src: file: <path>:<line>]`, `[src: url: <url>]`, or \
`[src: user:<id>:<date>: …]`. A citation that escapes the project root or \
points at a non-existent path is rejected as fabricated.

If something is a recommendation, opinion or guess — NOT a verified fact — say \
so: prefix with \"je recommande\" / \"I recommend\" / \"hypothèse:\", or tag it \
`[src: inferred: …]`. Honesty about uncertainty is never penalised.

Full cascade: `docs/AGENTS.md` § anti-hallucination.";

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
    pub fabricated_count: u32,
    /// **Niveau 1.5 soft signal.** Natural inline anchors (backticked
    /// `path:line`) the agent emitted that did NOT resolve. Distinct from
    /// `fabricated_count` (which is reserved for formal `[src:]` markers that
    /// failed — high confidence): an inline anchor that doesn't resolve is
    /// honestly "couldn't verify" (typo? cross-repo? wrong line?), surfaced as
    /// a soft amber pill, NOT a red "fabricated" one.
    #[serde(default)]
    pub unverified_count: u32,
}

impl LintReport {
    pub fn empty() -> Self {
        Self {
            unsourced_count: 0,
            flagged_spans: Vec::new(),
            sources: Vec::new(),
            fabricated_count: 0,
            unverified_count: 0,
        }
    }
    /// Count of mechanically-verified sources (file:line resolved on disk, or a
    /// trusted tier). Drives the GREEN positive pill. NOTE: "verified" means the
    /// source EXISTS / is anchored — NOT that the claim it backs is true.
    pub fn verified_count(&self) -> u32 {
        self.sources
            .iter()
            .filter(|s| s.status == SourceStatus::Verified)
            .count() as u32
    }

    /// True when the report is worth storing + showing a pill. Five tiers, all
    /// flowing from "a heuristic flag OR any citation at all":
    ///   - RED      fabricated — a formal `[src:]` didn't verify, OR
    ///   - AMBER    unsourced — a claim with no anchor (`unsourced_count`), OR
    ///   - AMBER-   unverified — an inline anchor that didn't resolve, OR
    ///   - GREEN    verified — a source resolved, OR
    ///   - NEUTRAL  unverifiable — only URL/user/inferred citations (can't be
    ///     machine-checked). **Option B (2026-05-30): we surface these too —
    ///     honesty over silence, "warn about everything".**
    ///
    /// Only a reply with NO heuristic flag AND NO citation of any kind is silent.
    pub fn has_signal(&self) -> bool {
        self.unsourced_count > 0 || !self.sources.is_empty()
    }

    /// Back-compat: "no signal at all". Note the flipped semantics vs the
    /// pre-0.8.8 version — an all-verified report is NO LONGER empty.
    pub fn is_empty(&self) -> bool {
        !self.has_signal()
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
    // ── Spanish — code / config (0.8.8 — closes the ES recall gap) ──
    //
    // Kronn is FR/EN/ES; before this block a genuine Spanish code claim
    // ("la función está definida en …") flagged via NOTHING (only language-
    // agnostic tokens like `cve-`/backticked-path fired). Same precision
    // discipline as the FR/EN blocks: tight verb-frames + state framings, no
    // generic prose. The matcher is accent-insensitive (normalize_match) so
    // "está" matches whether or not the accent survives.
    "se encuentra", "está definido", "está definida", "está implementado",
    "está almacenado", "está configurado", "está configurada", "por defecto",
    "devuelve", "la función", "el método", "la ruta", "la tabla", "la columna",
    "el parámetro", "la opción", "está declarado", "está gestionado",
    "es vulnerable", "está afectado por", "corregido en", "parcheado en",
    "versión afectada", "versiones afectadas", "la última versión es",
];

/// Hedges (EN + FR + ES). Their presence suppresses a flag — the agent is
/// already signalling uncertainty, which is exactly the behaviour we want.
const HEDGES: &[&str] = &[
    "i think", "i believe", "i'm not sure", "im not sure", "not sure",
    "maybe", "probably", "might be", "i should check", "unverified",
    "to verify", "let me check", "i'll check", "appears to", "seems to",
    "je pense", "je crois", "peut-être", "peut etre", "probablement",
    "il semble", "à vérifier", "a verifier", "je vérifie", "je verifie",
    "sans certitude", "je ne suis pas sûr", "je ne suis pas sur",
    // ── Spanish ──
    "creo que", "quizás", "quizas", "tal vez", "probablemente", "no estoy seguro",
    "parece que", "debería revisar", "por verificar", "sin certeza",
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

/// The single source-extension allowlist, shared by `contains_code_anchor`
/// (niveau-0: does a sentence carry a path anchor?) and `looks_like_file_anchor`
/// (niveau-1.5: is this backticked token a verifiable file path?). ONE list so
/// the two sites can't drift — a real bug a 4-persona forensic re-pass surfaced:
/// the two lists had diverged AND neither carried web-project extensions, so
/// every `.twig` / `.xlf` citation on a Symfony project went unverified (and the
/// sentences citing them read as "unsourced" → false positives too).
///
/// `.html.twig` (and any double extension) needs NO special case: matching is
/// `ends_with`, and `foo.html.twig` ends with `.twig`.
const SOURCE_EXTS: &[&str] = &[
    ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".toml", ".json", ".sql",
    ".md", ".yml", ".yaml", ".css", ".html", ".sh", ".php", ".go", ".java",
    ".rb", ".vue", ".svelte", ".c", ".cpp", ".h",
    // Web / template / i18n project files (Symfony, Laravel, Sass, XLIFF…).
    ".twig", ".xlf", ".scss", ".less",
];

/// Heuristic: a file path, a `path/like.ext` token, a namespaced identifier,
/// or a `fn()` call ref — any concrete, checkable reference.
fn contains_code_anchor(s: &str) -> bool {
    // File path with a known source extension (shared allowlist).
    const EXTS: &[&str] = SOURCE_EXTS;
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

/// Normalise a string for fuzzy cue/hedge matching: lowercase, fold the common
/// Latin accents, turn hyphens + curly apostrophes into their plain forms, and
/// collapse whitespace runs. This is what makes "peut être" == "peut-être" ==
/// "peut etre" — the accent+hyphen gap that let the DI false-positive through.
fn normalize_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let c = match ch {
            'à' | 'â' | 'ä' | 'á' | 'ã' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            '\u{2019}' => '\'', // curly apostrophe → straight
            '-' => ' ',
            c if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
            c => c,
        };
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Opinion / normative / planning frames. Their presence means the agent is
/// reasoning or recommending, not asserting a checkable fact — suppress the flag.
const OPINION_CUES: &[&str] = &[
    // ── French ──
    "peut etre", "pourrait etre", "n'est pas toujours", "pas forcement",
    "pas necessairement", "anti pattern", "il vaut mieux", "il faudrait",
    "on devrait", "tu devrais", "vous devriez", "devrait", "je recommande",
    "je recommanderais", "je suggere", "a mon avis", "selon moi",
    "ce serait mieux", "ferait mieux", "il serait preferable",
    // ── English ──
    "i recommend", "i suggest", "should be", "would be better",
    "in my opinion", "not always", "we should", "you should", "it would be",
    // ── Spanish (0.8.8 — precision guard for the new ES claim cues) ──
    "deberia", "seria mejor", "es preferible", "recomiendo", "sugiero",
    "en mi opinion", "a mi juicio", "no siempre", "mejor seria",
];

/// Conditional / hypothetical openers (bare tokens — matched at word boundaries
/// via [`find_first_word`], NOT as substrings, so "si" doesn't fire inside
/// "ver**si**on"). A claim cue that sits AFTER one of these in the same sentence
/// is hypothetical ("si X est géré par le DOM, …"), not an assertion.
const CONDITIONAL_OPENERS: &[&str] = &[
    "si", "s'il", "lorsque", "lorsqu'", "quand", "des que", "tant que",
    "a condition", "au cas ou", "supposons", "imaginons",
    "if", "when", "whenever", "assuming", "suppose",
    // ── Spanish ── ("si" already shared with FR)
    "cuando", "mientras", "en caso de", "supongamos",
];

/// First (earliest) needle hit in `haystack_norm`, with both sides normalised
/// via [`normalize_match`] so accented/hyphenated variants match. Returns the
/// byte position + the ORIGINAL needle (for the displayed reason).
fn find_first(haystack_norm: &str, needles: &[&str]) -> Option<(usize, String)> {
    needles
        .iter()
        .filter_map(|n| {
            let nn = normalize_match(n);
            haystack_norm.find(&nn).map(|p| (p, (*n).to_string()))
        })
        .min_by_key(|(p, _)| *p)
}

/// Like [`find_first`] but the needle must sit at WORD boundaries (preceded by
/// start-or-non-alphanumeric and followed by end-or-non-alphanumeric). Needed
/// for short cues like "si" / "if" that would otherwise match inside larger
/// words ("ver**si**on", "**if**ication"). Both sides normalised.
fn find_first_word(haystack_norm: &str, needles: &[&str]) -> Option<(usize, String)> {
    let bytes = haystack_norm.as_bytes();
    needles
        .iter()
        .filter_map(|n| {
            let nn = normalize_match(n);
            if nn.is_empty() {
                return None;
            }
            let mut from = 0usize;
            while let Some(rel) = haystack_norm[from..].find(&nn) {
                let start = from + rel;
                let end = start + nn.len();
                let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
                let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    return Some((start, (*n).to_string()));
                }
                from = start + 1;
            }
            None
        })
        .min_by_key(|(p, _)| *p)
}

/// A trailing-`?` sentence is a question, not an assertion.
fn is_question(s: &str) -> bool {
    s.trim_end().ends_with('?')
}

/// Markdown heading (`#…`, or a line ending in `**`) or an imperative-led bullet
/// ("Spécifier l'endpoint…", "Ajouter…") — a proposal/title, not a claim.
fn is_heading_or_imperative(s: &str) -> bool {
    let t = s.trim_start();
    if t.starts_with('#') || s.trim_end().ends_with("**") {
        return true;
    }
    let stripped = t.trim_start_matches(['-', '*', '•', ' ']);
    let first = stripped.split_whitespace().next().unwrap_or("");
    const IMPERATIVES: &[&str] = &[
        "specifier", "verifier", "ajouter", "creer", "definir", "configurer",
        "mettre", "faire", "voir", "utiliser", "remplacer", "supprimer",
        "add", "verify", "create", "define", "configure", "use", "replace",
        "remove", "check", "ensure",
    ];
    IMPERATIVES.contains(&normalize_match(first).as_str())
}

/// Strip fenced ```code blocks``` — agents emit lots of code and linting it
/// would be pure noise. Returns the text with fenced regions removed.
///
/// Fail-closed semantics (benchmark hotfix, 2026-07-21):
/// - a fence OPENS on a run of ≥3 backticks indented ≤3 spaces (info
///   string allowed); anything more indented or shorter is content;
/// - it CLOSES only on a run of the same character, at least as long,
///   indented ≤3 spaces, with a whitespace-only suffix — a ```suffix line
///   is CONTENT, never a closer;
/// - everything withheld (opening line included) is RESTORED verbatim if
///   the document ends before a valid closer: a truncated markdown must
///   never hide real content from a fail-closed consumer (the step-8
///   placeholder validator feeds on this).
pub(crate) fn strip_fenced_code(text: &str) -> String {
    /// `Some((run_len, suffix))` when the line is a fence marker candidate:
    /// ≤3 leading spaces then a run of ≥3 backticks. Tabs disqualify.
    fn fence_marker(line: &str) -> Option<(usize, &str)> {
        let stripped = line.trim_start_matches(' ');
        if line.len() - stripped.len() > 3 {
            return None; // 4+ spaces = indented code, not a fence
        }
        let run = stripped.chars().take_while(|&c| c == '`').count();
        if run < 3 {
            return None;
        }
        Some((run, &stripped[run..]))
    }

    let mut out = String::with_capacity(text.len());
    // (opening run length, withheld lines — restored verbatim on EOF)
    let mut fence: Option<(usize, String)> = None;
    for line in text.lines() {
        match fence.as_mut() {
            None => {
                // A backtick fence's info string may not contain a backtick
                // (CommonMark): ```lang`oops is CONTENT, not an opener — a
                // false opener must never swallow a real slot.
                match fence_marker(line) {
                    Some((run, info)) if !info.contains('`') => {
                        let mut buf = String::new();
                        buf.push_str(line);
                        buf.push('\n');
                        fence = Some((run, buf));
                    }
                    _ => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
            Some((open_run, buf)) => match fence_marker(line) {
                // Closer suffix: spaces/tabs ONLY — Unicode whitespace must
                // not promote a content line into a closer.
                Some((run, suffix))
                    if run >= *open_run
                        && suffix.chars().all(|c| c == ' ' || c == '\t') =>
                {
                    fence = None; // valid closer: the withheld block is code — drop it
                }
                _ => {
                    buf.push_str(line);
                    buf.push('\n');
                }
            },
        }
    }
    if let Some((_, buf)) = fence {
        out.push_str(&buf);
    }
    out
}

/// Lines with more backtick runs than this keep their spans (degraded =
/// pre-quick-win behaviour) — bounds the pairing work on adversarial input.
const MAX_BACKTICK_RUNS_PER_LINE: usize = 64;

/// Strip `inline code` spans — a marker QUOTED in backticks is the agent
/// talking ABOUT the `[src:]` syntax, not citing (pre-tag quick win: this
/// false-positived 4× in one live session as "fabricated"). Strictly
/// lexical, per line (markdown inline code doesn't survive a newline): a
/// span opens with a run of N backticks and closes at the next run of
/// exactly N (so `` `a` `` and ``` ``a`` ``` both work); an unclosed run is
/// kept verbatim — backticks in plain prose never eat the rest of the line.
/// Runs are pre-scanned once per line and their count is capped, so a long
/// hostile line can't turn the pairing into a CPU hotspot (Copilot review).
pub(crate) fn strip_inline_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        // Fast path — the overwhelmingly common case.
        if !line.contains('`') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        // One pass: every backtick run as (start, len).
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut k = 0usize;
        while k < chars.len() {
            if chars[k] == '`' {
                let start = k;
                while k < chars.len() && chars[k] == '`' {
                    k += 1;
                }
                runs.push((start, k - start));
            } else {
                k += 1;
            }
        }
        if runs.len() > MAX_BACKTICK_RUNS_PER_LINE {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // Pair each opener with the next run of EXACTLY its length; runs
        // consumed inside a span are never reconsidered as openers.
        let mut drop_ranges: Vec<(usize, usize)> = Vec::new(); // [start, end)
        let mut ri = 0usize;
        while ri < runs.len() {
            let (start, n) = runs[ri];
            match (ri + 1..runs.len()).find(|&j| runs[j].1 == n) {
                Some(rj) => {
                    let (cstart, clen) = runs[rj];
                    drop_ranges.push((start, cstart + clen));
                    ri = rj + 1;
                }
                None => ri += 1, // unclosed — kept verbatim
            }
        }
        let mut i = 0usize;
        let mut di = 0usize;
        while i < chars.len() {
            if di < drop_ranges.len() && i == drop_ranges[di].0 {
                // A single space where the span was — the surrounding prose
                // must never weld into a NEW `[src:` across the boundary
                // (Copilot: "[s" + span + "rc: …]" would mint a citation).
                out.push(' ');
                i = drop_ranges[di].1;
                di += 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out.push('\n');
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
                // Retain a terminating `?`/`!` on the flushed sentence so the
                // downstream `is_question` check still sees it. Without this the
                // delimiter was consumed as a boundary and stripped, making
                // `is_question` (which checks `ends_with('?')`) dead code — a
                // genuine interrogative with a claim cue (esp. French "… ?",
                // where the space-before-? is standard typography) wrongly
                // flagged. A `.` is NOT retained: it carries no signal and a
                // sentence-final period would only add noise to excerpts.
                if matches!(ch, '!' | '?') {
                    cur.push(ch);
                }
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
        let nmatch = normalize_match(&sentence); // accent/hyphen-folded
        // Hedged OR opinion/recommendation frame → the agent is signalling
        // uncertainty or reasoning, not asserting a checkable fact. Good.
        // Opinion cues use word-boundary matching (some are short tokens).
        if find_first(&nmatch, HEDGES).is_some() || find_first_word(&nmatch, OPINION_CUES).is_some() {
            continue;
        }
        // Anchored → self-sourcing.
        if has_anchor(&lower, &sentence) {
            continue;
        }
        // Questions, headings and imperative bullets are not assertions.
        if is_question(&sentence) || is_heading_or_imperative(&sentence) {
            continue;
        }
        if let Some((cue_pos, cue)) = find_first(&nmatch, CLAIM_CUES) {
            // Conditional guard: a cue sitting AFTER "si/quand/if…" is
            // hypothetical ("si le cycle est géré par le DOM, …"), not a claim.
            // Word-boundary match so "si" doesn't fire inside "version".
            if let Some((cond_pos, _)) = find_first_word(&nmatch, CONDITIONAL_OPENERS) {
                if cond_pos < cue_pos {
                    continue;
                }
            }
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
        unverified_count: 0,
    }
}

// ─── Niveau 1 — structured source extraction + mechanical verification ────

/// Hard caps to bound work at message-finalize time.
const MAX_SOURCES_VERIFIED: usize = 50;
/// Skip line-count verification above this file size (existence still checked).
const LINE_COUNT_SIZE_CAP_BYTES: u64 = 2 * 1024 * 1024;

/// Extract every `[src: …]` marker from the text, skipping fenced code blocks
/// AND inline-backtick spans (example/quoted syntax shouldn't be verified),
/// and dropping EMPTY markers — a bare `[src:]` in prose is a mention of the
/// grammar, not a citation. Returns the inner content (after `src:`),
/// trimmed. Bracket-balanced so `[src: a[0]]` reads `a[0]`.
pub fn extract_source_markers(text: &str) -> Vec<String> {
    let prose = strip_inline_code(&strip_fenced_code(text));
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
            let mut closed = false;
            while j < chars.len() {
                match chars[j] {
                    '[' => {
                        depth += 1;
                        buf.push('[');
                    }
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                        buf.push(']');
                    }
                    c => buf.push(c),
                }
                j += 1;
            }
            // Only a CLOSED marker is a citation — partially-typed prose
            // (`[src: foo.rs:1` at end of text) must not reach verification
            // (Copilot review on PR 119).
            if closed {
                let inner = buf.trim();
                if !inner.is_empty() {
                    out.push(inner.to_string());
                }
            }
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
    let roots: Vec<&Path> = project_root.into_iter().collect();
    verify_source_marker_roots(raw, &roots)
}

/// Multi-root variant — file refs are resolved against the FIRST root where they
/// exist (worktree before main checkout). `Option`-based [`verify_source_marker`]
/// delegates here with a 0-or-1 element slice.
pub fn verify_source_marker_roots(raw: &str, roots: &[&Path]) -> SourceCheck {
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
            let (st, detail) = verify_file_ref(&reference, roots);
            (st, format!("code comment (not authoritative — verify): {}", detail))
        }
        SourceKind::File => verify_file_ref(&reference, roots),
    };

    SourceCheck {
        raw: raw.trim().to_string(),
        kind,
        status: status_detail.0,
        detail: status_detail.1,
    }
}

/// Strip the wrappers agents routinely put around an inline path citation:
/// surrounding backticks / quotes / angle / round brackets, and trailing
/// sentence punctuation. Pure string hygiene — it never ADDS path components,
/// so it can't widen the path-escape (SSRF) surface. This is the fix for the
/// "plein de fichiers non trouvés" bug: a perfectly valid `` `src/foo.rs:42` ``
/// used to fail because the backticks blocked both the line-spec peel and the
/// existence check.
fn clean_reference(reference: &str) -> &str {
    let mut s = reference.trim();
    loop {
        let before = s;
        s = s.trim_matches(['`', '"', '\'', '<', '>', '(', ')']).trim();
        s = s.trim_end_matches(['.', ',', ';']).trim();
        if s == before {
            break;
        }
    }
    s
}

/// Line-bounds verdict for an existing candidate file.
fn line_bounds_status(candidate: &Path, line_spec: Option<(usize, usize)>) -> (SourceStatus, String) {
    match line_spec {
        None => (SourceStatus::Verified, "file exists".into()),
        Some((start, end)) => match count_lines_capped(candidate) {
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

/// Heavy / generated / Kronn-internal dirs never descended when resolving a
/// bare basename. Mirrors `scanner::scan_kronn_markers`'s skip list. Skipping
/// `.kronn` is load-bearing: its `worktrees/` hold FULL project copies, so
/// without it every basename looks ambiguous — the exact false-ambiguity that
/// masked the real unique file in front_euronews (11 copies, 1 real).
const BASENAME_WALK_SKIP_DIRS: &[&str] = &[
    "node_modules", "vendor", "target", ".git", "dist", "build", ".next",
    ".kronn", ".kronn-worktrees", ".venv", "__pycache__",
];

/// Outcome of walking the tree for a bare basename (no path separator).
enum BasenameResolution {
    /// Exactly one file under the (first matching) root carries this basename.
    Unique(PathBuf),
    /// Two or more candidates — too ambiguous to green-light.
    Ambiguous(usize),
    /// No file with this basename, or the walk was capped before deciding.
    NotFound,
}

/// Resolve a bare basename (e.g. `Foo.ts`) to a UNIQUE file in the project
/// tree, so an agent that cites `Foo.ts:42` without the full path stops being a
/// false "unverified". Roots are tried IN ORDER (Isolated worktree before the
/// main checkout, mirroring `verify_file_ref`), and the FIRST root that holds
/// any match decides — so the same file present in both a worktree root and the
/// main root is not double-counted as ambiguous. Heavy / `.kronn` dirs are
/// pruned. Bounded: past `MAX_WALK_ENTRIES` we bail to `NotFound` rather than
/// risk a false `Unique` on a partial scan.
fn resolve_unique_basename(basename: &str, roots: &[&Path]) -> BasenameResolution {
    const MAX_WALK_ENTRIES: usize = 60_000;
    for root in roots {
        let mut found: Option<PathBuf> = None;
        let mut count = 0usize;
        let mut scanned = 0usize;
        let mut capped = false;
        let walker = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                // Prune skip dirs by name (depth > 0 so the root itself stays).
                if e.depth() > 0 && e.file_type().is_dir() {
                    if let Some(name) = e.file_name().to_str() {
                        return !BASENAME_WALK_SKIP_DIRS.contains(&name);
                    }
                }
                true
            });
        for entry in walker.filter_map(|e| e.ok()) {
            scanned += 1;
            if scanned > MAX_WALK_ENTRIES {
                capped = true;
                break;
            }
            if entry.file_type().is_file() && entry.file_name().to_str() == Some(basename) {
                count += 1;
                if count == 1 {
                    found = Some(entry.path().to_path_buf());
                } else {
                    return BasenameResolution::Ambiguous(count); // ≥2 in this root
                }
            }
        }
        if capped {
            return BasenameResolution::NotFound; // don't claim a unique on a partial scan
        }
        if let Some(p) = found {
            return BasenameResolution::Unique(p); // exactly one in this root → decided
        }
        // 0 matches in this root → fall through to the next root.
    }
    BasenameResolution::NotFound
}

/// True iff `full` ends with `needle` at a PATH-SEGMENT boundary: either equal,
/// or `full` ends with `"/" + needle`. Segment alignment is what stops
/// `Foo.php` matching `BarFoo.php` and `apps/x` matching `myapps/x`.
fn path_has_segment_suffix(full: &str, needle: &str) -> bool {
    if full == needle {
        return true;
    }
    full.strip_suffix(needle).is_some_and(|rest| rest.ends_with('/'))
}

/// Resolve a MULTI-SEGMENT relative citation to a UNIQUE file by segment-aligned
/// full-path suffix match. Fixes the dominant inline-anchor false positive: a
/// path cited relative to an app SUBDIR (project code lives under `application/`
/// but the verify root is the project root → `apps/x/Foo.php` is really at
/// `application/apps/x/Foo.php`), or a sibling-repo path written with the repo
/// name (`front_apollo/plugins/…`, resolved once that linked repo is a root).
///
/// Guards mirror `resolve_unique_basename` — heavy/`.kronn` dirs pruned (the
/// worktree copies would otherwise make everything ambiguous), walk capped, and
/// ONLY a unique match is green-lit (2+ → Ambiguous, never a false verify). The
/// match is always a real descendant of a registered root, so no SSRF/escape.
fn resolve_unique_path_suffix(needle: &str, roots: &[&Path]) -> BasenameResolution {
    const MAX_WALK_ENTRIES: usize = 60_000;
    let needle = needle.replace('\\', "/");
    let needle = needle.trim_start_matches('/');
    // Single-segment needles are the basename walk's job; require ≥2 segments.
    if needle.is_empty() || !needle.contains('/') {
        return BasenameResolution::NotFound;
    }
    for root in roots {
        let mut found: Option<PathBuf> = None;
        let mut count = 0usize;
        let mut scanned = 0usize;
        let mut capped = false;
        let walker = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| {
                if e.depth() > 0 && e.file_type().is_dir() {
                    if let Some(name) = e.file_name().to_str() {
                        return !BASENAME_WALK_SKIP_DIRS.contains(&name);
                    }
                }
                true
            });
        for entry in walker.filter_map(|e| e.ok()) {
            scanned += 1;
            if scanned > MAX_WALK_ENTRIES {
                capped = true;
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let full = entry.path().to_string_lossy().replace('\\', "/");
            if path_has_segment_suffix(&full, needle) {
                count += 1;
                if count == 1 {
                    found = Some(entry.path().to_path_buf());
                } else {
                    return BasenameResolution::Ambiguous(count);
                }
            }
        }
        if capped {
            return BasenameResolution::NotFound;
        }
        if let Some(p) = found {
            return BasenameResolution::Unique(p);
        }
    }
    BasenameResolution::NotFound
}

/// Verify a file reference against one or more candidate roots. The FIRST root
/// where the (jailed) relative path exists wins — this is how an Isolated
/// discussion's git worktree is tried before the main checkout, fixing the
/// false-NotFound on files the agent saw/created in the worktree.
///
/// - Absolute path → existence-only (2026-05-28 decision: no jail, no symlink
///   check; the agent emits a fully-qualified path and it either exists or not).
/// - Relative path → lexical jail + symlink-escape re-check under EACH root.
///   `../` escape in every root → OutsideProject; jailed-but-absent everywhere
///   → NotFound. The SSRF/jail guarantee is unchanged (applied per-root).
/// - Bare basename (no separator) unresolved at root level → unique-match walk
///   (`resolve_unique_basename`) before NotFound.
fn verify_file_ref(reference: &str, roots: &[&Path]) -> (SourceStatus, String) {
    let reference = clean_reference(reference);
    if reference.is_empty() {
        return (SourceStatus::EmptyRef, "empty source reference".into());
    }
    if roots.is_empty() {
        return (
            SourceStatus::Unchecked,
            "no project root — can't resolve file path".into(),
        );
    }

    let (path_str, line_spec) = split_path_and_lines(reference);
    // Re-clean: the FP fix. Agents write `path`:line (markdown path in
    // backticks, the :line OUTSIDE them). At the top, `clean_reference` only
    // peels OUTER wrappers — the closing backtick sits internal (before :line)
    // and survives; `split_path_and_lines` then appends it onto path_str, so a
    // real file fails to stat. Re-cleaning the post-split path strips it. Still
    // trim-only (returns a &str sub-slice) so it can never add a `../`
    // component — SSRF-safe. Empty after the peel = an empty ref.
    let path_str = clean_reference(path_str);
    if path_str.is_empty() {
        return (SourceStatus::EmptyRef, "empty source reference".into());
    }
    let path = Path::new(path_str);

    // Absolute → existence-only, host-path-translated for Docker. No jail.
    if path.is_absolute() {
        let candidate = crate::core::scanner::resolve_host_path(path_str);
        if !candidate.exists() {
            return (SourceStatus::NotFound, format!("file not found: {}", path_str));
        }
        return line_bounds_status(&candidate, line_spec);
    }

    // Relative → try each root. Lexical jail + symlink re-check per root.
    let mut all_escaped = true;
    for root in roots {
        let joined = root.join(path);
        let norm_joined = normalize_lexical(&joined);
        let norm_root = normalize_lexical(root);
        if !norm_joined.starts_with(&norm_root) {
            // Escapes THIS root via `../` — try the next one before deciding.
            continue;
        }
        all_escaped = false;
        if !joined.exists() {
            continue;
        }
        // Symlink escape guard: a `subdir/` symlink to `/etc/` passes the
        // lexical jail but canonicalises outside — re-check on disk.
        if let (Ok(canon), Ok(canon_root)) = (joined.canonicalize(), root.canonicalize()) {
            if !canon.starts_with(&canon_root) {
                return (
                    SourceStatus::OutsideProject,
                    "resolves (via symlink) outside the project root".into(),
                );
            }
        }
        return line_bounds_status(&joined, line_spec);
    }

    if all_escaped {
        return (
            SourceStatus::OutsideProject,
            "relative path escapes the project root via ../".into(),
        );
    }

    // Bare basename (no separator) that didn't resolve at any root level — it
    // very likely lives deeper in the tree. Walk for a UNIQUE basename match
    // before giving up. This kills the dominant false positive: an agent citing
    // `Foo.ts:42` without the full path was flagged "unverified" even though the
    // file exists. Only a UNIQUE match is green-lit; 2+ stays unresolved (we
    // never claim a fact we can't pin), but with an actionable reason.
    if !path_str.is_empty() && !path_str.contains('/') && !path_str.contains('\\') {
        match resolve_unique_basename(path_str, roots) {
            BasenameResolution::Unique(found) => {
                let (status, detail) = line_bounds_status(&found, line_spec);
                let shown = roots
                    .iter()
                    .find_map(|r| found.strip_prefix(r).ok())
                    .unwrap_or(found.as_path())
                    .display();
                return (status, format!("{detail} — resolved by unique basename → {shown}"));
            }
            BasenameResolution::Ambiguous(n) => {
                return (
                    SourceStatus::NotFound,
                    format!("bare name `{path_str}` matches {n} files — ambiguous, cite the full path"),
                );
            }
            BasenameResolution::NotFound => {}
        }
    } else {
        // Multi-segment relative path unresolved at root level — it may live
        // deeper (cited relative to an app subdir, e.g. `apps/x/Foo.php` actually
        // at `application/apps/x/Foo.php`) or in a sibling repo by name
        // (`front_apollo/plugins/…`). Walk for a UNIQUE segment-aligned suffix
        // match across the roots (incl. linked_repos) before giving up.
        match resolve_unique_path_suffix(path_str, roots) {
            BasenameResolution::Unique(found) => {
                let (status, detail) = line_bounds_status(&found, line_spec);
                let shown = roots
                    .iter()
                    .find_map(|r| found.strip_prefix(r).ok())
                    .unwrap_or(found.as_path())
                    .display();
                return (status, format!("{detail} — resolved by unique path suffix → {shown}"));
            }
            BasenameResolution::Ambiguous(n) => {
                return (
                    SourceStatus::NotFound,
                    format!("path `{path_str}` matches {n} files — ambiguous, cite the full path"),
                );
            }
            BasenameResolution::NotFound => {}
        }
    }

    (SourceStatus::NotFound, format!("file not found: {}", path_str))
}

/// Full P2 analysis: niveau 0 heuristic + niveau 1 mechanical source
/// verification. `project_root` is the disc's effective working tree (project
/// path, or worktree path in test/isolated mode); `None` for project-less discs
/// (file refs become `Unchecked`).
pub fn analyze(text: &str, project_root: Option<&Path>) -> LintReport {
    let roots: Vec<&Path> = project_root.into_iter().collect();
    analyze_roots(text, &roots)
}

/// Extract the backticked file-path anchors agents emit NATURALLY instead of a
/// formal `[src:]` marker — `` `path/file.ext` `` or `` `path/file.ext:line` ``.
/// High-precision on purpose: a slash AND a known source extension are both
/// required, so backticked identifiers / calls / bare prose are NOT treated as
/// file refs (no new red false-positives). Fenced code is skipped.
fn extract_inline_file_anchors(text: &str) -> Vec<String> {
    let prose = strip_fenced_code(text);
    let mut out = Vec::new();
    let mut in_tick = false;
    let mut buf = String::new();
    for ch in prose.chars() {
        if ch == '`' {
            if in_tick {
                let t = buf.trim();
                if looks_like_file_anchor(t) {
                    out.push(t.to_string());
                }
                buf.clear();
            }
            in_tick = !in_tick;
        } else if in_tick {
            buf.push(ch);
        }
    }
    out
}

fn looks_like_file_anchor(s: &str) -> bool {
    if s.is_empty() || s.contains(char::is_whitespace) {
        return false; // a path token carries no spaces
    }
    let (path, line) = split_path_and_lines(s);
    let lower = path.to_ascii_lowercase();
    if !SOURCE_EXTS.iter().any(|e| lower.ends_with(e)) {
        return false; // must end in a known source extension
    }
    // Plus a strong file signal: a path separator OR an explicit `:line`. The
    // separator catches `src/foo.rs`; the line spec catches ROOT-LEVEL files
    // cited with a line — `composer.json:1`, `Cargo.toml:5`, `README.md:3` —
    // which previously fell through (a real recall gap a live test surfaced).
    // Bare extensionful prose with neither (`node.js`) stays out → precision kept.
    path.contains('/') || line.is_some()
}

/// Bare-path dedup key for a citation: strip the optional `<type>:` prefix,
/// the citation wrappers, and the `:line` spec — so a `[src: file: a/b.rs:2]`
/// marker and an inline `` `a/b.rs:5` `` resolve to the same key (`a/b.rs`).
fn file_path_key(raw: &str) -> String {
    let (_, reference) = classify_source(raw);
    let cleaned = clean_reference(&reference);
    let (path, _) = split_path_and_lines(cleaned);
    path.to_string()
}

/// Full P2 analysis over one or more candidate roots. `roots` is the disc's
/// effective working tree(s) — the Isolated worktree first, then the project
/// path — empty for project-less discs (file refs become `Unchecked`).
///
/// Four signals: niveau-0 prose heuristic, niveau-1 mechanical `[src:]`
/// verification (→ `fabricated_count`, RED, high-confidence), and niveau-1.5 of
/// NATURAL backticked path anchors — which resolve (→ verified, GREEN) OR don't
/// (→ `unverified_count`, soft AMBER). Honesty principle: a non-resolving inline
/// anchor is NOT silently dropped (the old behaviour hid wrong inline citations)
/// and NOT escalated to red "fabricated" (it's lower confidence than a formal
/// `[src:]` — could be a typo / cross-repo / wrong line). It's surfaced as
/// "couldn't verify", honestly.
pub fn analyze_roots(text: &str, roots: &[&Path]) -> LintReport {
    let mut report = lint_assertions(text);
    let mut fabricated = 0u32;
    let mut unverified = 0u32;
    let mut sources = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw in extract_source_markers(text).into_iter().take(MAX_SOURCES_VERIFIED) {
        let check = verify_source_marker_roots(&raw, roots);
        if check.status.is_fabricated() {
            fabricated += 1;
        }
        // Dedup inline anchors against [src:] file refs by their bare PATH (no
        // line spec, no type prefix) — otherwise `[src: file: a.rs:2]` and an
        // inline `` `a.rs:5` `` would both count as verified and double the
        // green chip / inflate the telemetry.
        if matches!(check.kind, SourceKind::File | SourceKind::CodeComment) {
            seen.insert(file_path_key(&raw));
        }
        sources.push(check);
    }
    // Niveau 1.5 — natural backticked path anchors. Resolve → verified (green);
    // don't resolve → surfaced honestly as "unverified" (soft amber), NOT red.
    if !roots.is_empty() {
        for anchor in extract_inline_file_anchors(text).into_iter().take(MAX_SOURCES_VERIFIED) {
            let key = file_path_key(&anchor);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            let (status, detail) = verify_file_ref(&anchor, roots);
            if status == SourceStatus::Verified {
                sources.push(SourceCheck {
                    raw: anchor,
                    kind: SourceKind::File,
                    status,
                    detail: format!("inline anchor (auto-verified): {}", detail),
                });
            } else {
                // Didn't resolve: honest soft signal, not a red fabrication.
                // The `Unchecked` status keeps it OUT of the red "bad sources"
                // bucket (which keys on is_fabricated) while still listing it.
                unverified += 1;
                sources.push(SourceCheck {
                    raw: anchor,
                    kind: SourceKind::File,
                    status: SourceStatus::Unchecked,
                    detail: format!("inline anchor (couldn't verify): {}", detail),
                });
            }
        }
    }
    report.sources = sources;
    report.fabricated_count = fabricated;
    report.unverified_count = unverified;
    report
}

/// Build the lint report to attach to a finalized agent message. Assembles the
/// candidate roots (Isolated worktree FIRST, then the project checkout — both
/// host-path-translated for Docker), runs the full analysis, and keeps the
/// report only when it carries a signal (green/amber/red); `None` ⇒ nothing to
/// show. Returns `None` immediately when the feature is off.
///
/// Also emits ONE structured `anti_halluc`-target log line with the scalar
/// counts, so the false-positive / verified-anchor rate can be measured from
/// production logs without reaching the DB — the P4 heuristic is meant to be
/// tuned from real data, and this is the only telemetry hook.
///
/// Extracted from the `make_agent_stream` finalize closure so this seam (roots
/// assembly + has_signal gate) is unit-testable — it was the lowest-covered,
/// highest-blast-radius part of the anti-hallu path.
pub fn finalize_lint_report(
    text: &str,
    workspace_path: Option<&str>,
    project_path: &str,
    linked_repo_paths: &[String],
) -> Option<LintReport> {
    if !current_mode().is_active() {
        return None;
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(wp) = workspace_path.filter(|w| !w.is_empty()) {
        roots.push(crate::core::scanner::resolve_host_path(wp));
    }
    if !project_path.is_empty() {
        roots.push(crate::core::scanner::resolve_host_path(project_path));
    }
    // 0.8.8 — also resolve citations against the project's declared linked_repos
    // (filesystem locations only). Triage/dev agents legitimately cite sibling
    // repos they were told about (front_apollo, …); without these as roots a
    // real cross-repo path reads as "couldn't verify". resolve_host_path-
    // translated; kept only if it actually exists in-container (repos under
    // $HOME are mounted at /host-home), else skipped so a genuinely-unavailable
    // repo's refs stay soft-unverified rather than erroring.
    for lp in linked_repo_paths {
        if lp.is_empty() {
            continue;
        }
        let resolved = crate::core::scanner::resolve_host_path(lp);
        if resolved.exists() && !roots.contains(&resolved) {
            roots.push(resolved);
        }
    }
    let root_refs: Vec<&Path> = roots.iter().map(|p| p.as_path()).collect();
    let report = analyze_roots(text, &root_refs);
    tracing::info!(
        target: "anti_halluc",
        unsourced = report.unsourced_count,
        fabricated = report.fabricated_count,
        verified = report.verified_count(),
        roots = roots.len(),
        "lint finalized",
    );
    if report.has_signal() {
        Some(report)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fenced_code_drops_closed_blocks_but_restores_unclosed_ones() {
        // Closed fence: content is code, dropped.
        assert_eq!(
            strip_fenced_code("before\n```\ncode\n```\nafter"),
            "before\nafter\n"
        );
        // UNCLOSED fence (truncated document): the withheld content is
        // restored — a fail-closed consumer (step-8 placeholder validator)
        // must never lose sight of real content behind a stray ```.
        assert_eq!(
            strip_fenced_code("```text\ntruncated example\n| {{ID}} |"),
            "```text\ntruncated example\n| {{ID}} |\n"
        );
        // Two blocks, second unclosed: first dropped, second restored
        // (including ITS opening line).
        assert_eq!(
            strip_fenced_code("```\na\n```\nkeep\n```\nb"),
            "keep\n```\nb\n"
        );
        // The opening line itself is withheld-then-restored: an info-string
        // carrying real content must survive an unclosed fence.
        assert_eq!(
            strip_fenced_code("```text {{ID}}\ntruncated"),
            "```text {{ID}}\ntruncated\n"
        );
        // A ```suffix line is CONTENT, not a closer — without a valid
        // closer the whole block is restored.
        assert_eq!(
            strip_fenced_code("```text\n| {{ID}} |\n```still-code"),
            "```text\n| {{ID}} |\n```still-code\n"
        );
        // A closer run SHORTER than the opening run does not close.
        assert_eq!(
            strip_fenced_code("````\n| {{ID}} |\n```\ntail"),
            "````\n| {{ID}} |\n```\ntail\n"
        );
        // 4-space indentation = indented code, not a fence at all.
        assert_eq!(
            strip_fenced_code("    ```\n| {{ID}} |"),
            "    ```\n| {{ID}} |\n"
        );
        // Trailing whitespace on a closer is fine (CommonMark).
        assert_eq!(strip_fenced_code("```\ncode\n```   "), "");
        // A backtick in the info string invalidates the OPENER: the line is
        // content and nothing after it is withheld.
        assert_eq!(
            strip_fenced_code("```lang`oops\n| {{ID}} |\n```"),
            "```lang`oops\n| {{ID}} |\n```\n"
        );
        // Unicode whitespace in a closer suffix does NOT close (spaces and
        // tabs only) — the block stays unclosed and is restored.
        assert_eq!(
            strip_fenced_code("```\n| {{ID}} |\n```\u{00A0}"),
            "```\n| {{ID}} |\n```\u{00A0}\n"
        );
    }

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

    // ── PR-B enforce-mode disc helpers ────────────────────────────────

    #[test]
    fn auto_attach_doc_author_only_in_enforce_with_agents_md() {
        let none: Vec<String> = vec![];
        // Enforce + project has AGENTS.md + not already attached → attach.
        assert!(should_auto_attach_doc_author(AntiHallucMode::Enforce, &none, true));
        // Wrong mode → never.
        assert!(!should_auto_attach_doc_author(AntiHallucMode::Warn, &none, true));
        assert!(!should_auto_attach_doc_author(AntiHallucMode::Off, &none, true));
        // No docs/AGENTS.md → never (nothing to discipline against).
        assert!(!should_auto_attach_doc_author(AntiHallucMode::Enforce, &none, false));
        // Idempotent: already attached → don't duplicate.
        let attached = vec!["rust".to_string(), "kronn-doc-author".to_string()];
        assert!(!should_auto_attach_doc_author(AntiHallucMode::Enforce, &attached, true));
    }

    #[test]
    fn enforce_refusal_only_when_enforce_and_fabricated() {
        assert!(enforce_refusal_needed(AntiHallucMode::Enforce, 1));
        assert!(enforce_refusal_needed(AntiHallucMode::Enforce, 7));
        // No fabricated citations → no refusal even in enforce.
        assert!(!enforce_refusal_needed(AntiHallucMode::Enforce, 0));
        // Warn/off never refuse, regardless of count.
        assert!(!enforce_refusal_needed(AntiHallucMode::Warn, 3));
        assert!(!enforce_refusal_needed(AntiHallucMode::Off, 3));
    }

    #[test]
    fn refusal_message_states_count_and_non_destructive() {
        let msg = enforce_refusal_message(2);
        assert!(msg.contains('2'));
        assert!(msg.to_lowercase().contains("refus"));
        // Must convey the message is KEPT (non-destructive), not deleted.
        assert!(msg.contains("conservée"));
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

    #[serial_test::serial] // mutates the process-global anti-halluc mode
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

    // ── Pre-tag quick win — literal-mention false positives ──────────
    // Live incident (2026-07-14 room, 4×): TALKING about the `[src:]`
    // grammar in backticks was linted as a fabricated citation.

    #[test]
    fn extract_ignores_a_syntax_mention_in_backticks() {
        let txt = "la citation `[src:]` est obligatoire dans la doc.";
        assert!(extract_source_markers(txt).is_empty(), "quoted syntax is a mention, not a citation");
    }

    #[test]
    fn extract_ignores_a_full_quoted_marker_in_inline_code() {
        // Codex non-regression ask: literal text that LOOKS like a
        // provenance, quoted in inline code — must extract nothing.
        let txt = "utilise `[src: file: main.rs:10]` pour citer une ligne.";
        assert!(extract_source_markers(txt).is_empty());
        let double = "ou ``[src: url: https://x.com]`` en double backticks.";
        assert!(extract_source_markers(double).is_empty());
    }

    #[test]
    fn extract_drops_an_empty_marker_in_prose() {
        assert!(extract_source_markers("un [src:] nu est une mention de grammaire.").is_empty());
        assert!(extract_source_markers("avec espaces [src:   ] aussi.").is_empty());
    }

    #[test]
    fn extract_keeps_the_real_marker_next_to_a_quoted_mention() {
        let txt = "La grammaire `[src:]` s'applique ici [src: foo.rs:1].";
        assert_eq!(extract_source_markers(txt), vec!["foo.rs:1"]);
    }

    #[test]
    fn extract_survives_an_unclosed_backtick() {
        // A lone backtick in prose must not swallow the rest of the line.
        let txt = "un ` isolé puis une vraie citation [src: real.rs:2] ensuite.";
        assert_eq!(extract_source_markers(txt), vec!["real.rs:2"]);
    }

    #[test]
    fn extract_drops_an_unclosed_marker() {
        // Copilot (PR 119): partially-typed prose must not become a
        // citation — no closing `]` means no marker.
        assert!(extract_source_markers("truncated at end [src: foo.rs:1").is_empty());
        let mixed = "closed [src: a.rs:1] then truncated [src: b.rs:9";
        assert_eq!(extract_source_markers(mixed), vec!["a.rs:1"]);
    }

    #[test]
    fn extract_does_not_weld_prose_across_a_dropped_span() {
        // Copilot (PR 119 round 3): "[s" + `span` + "rc: …]" must not
        // concatenate into a fresh "[src:" once the span is removed — the
        // span leaves a space behind.
        let txt = "des crochets [s`et du code`rc: foo.rs:1] ne fusionnent pas.";
        assert!(extract_source_markers(txt).is_empty());
        let with_real = "idem [s`x`rc: nope] mais [src: ok.rs:4] reste.";
        assert_eq!(extract_source_markers(with_real), vec!["ok.rs:4"]);
    }

    #[test]
    fn extract_is_bounded_on_a_backtick_heavy_line() {
        // Copilot (PR 119): > MAX_BACKTICK_RUNS_PER_LINE runs on one line
        // skips span-stripping (degraded = pre-quick-win behaviour) instead
        // of pairing runs — the call stays cheap and markers outside spans
        // still extract.
        let noise = "` x ".repeat(100); // 100 unmatched single-backtick runs
        let txt = format!("{noise} et [src: real.rs:3] à la fin.");
        assert_eq!(extract_source_markers(&txt), vec!["real.rs:3"]);
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

    // ── Bare-basename resolution (kills the "Foo.ts:42 unverified" FP) ──

    /// Make a fresh temp project root and run `f` to populate it. Returns root.
    fn temp_root_with(f: impl FnOnce(&Path)) -> PathBuf {
        let mut d = std::env::temp_dir();
        d.push(format!("kronn_basename_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        f(&d);
        d
    }

    fn write_file(root: &Path, rel: &str, lines: usize) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x\n".repeat(lines)).unwrap();
    }

    #[test]
    fn bare_basename_resolves_to_unique_nested_file() {
        // The core fix: a bare `Widget.ts:2` cited without its full path, living
        // deep in the tree, now verifies instead of showing "unverified".
        let root = temp_root_with(|r| write_file(r, "app/assets/ts/Services/Widget.ts", 3));
        let (status, detail) = verify_file_ref("Widget.ts:2", &[&root]);
        assert_eq!(status, SourceStatus::Verified, "{detail}");
        assert!(detail.contains("unique basename"), "{detail}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_basename_skips_dot_kronn_worktree_copies() {
        // The real front_euronews case: 1 real file + a copy under
        // `.kronn/worktrees/`. Without the skip the basename looks ambiguous;
        // with it, the main copy is the unique match → Verified.
        let root = temp_root_with(|r| {
            write_file(r, "app/assets/ts/Widget.ts", 3);
            write_file(r, ".kronn/worktrees/wt-abc/app/assets/ts/Widget.ts", 3);
            write_file(r, "node_modules/pkg/Widget.ts", 3);
        });
        let (status, _d) = verify_file_ref("Widget.ts:1", &[&root]);
        assert_eq!(status, SourceStatus::Verified, "{_d}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_basename_two_real_copies_stay_ambiguous() {
        // Two genuine copies (NOT under a skip dir) → we refuse to guess.
        let root = temp_root_with(|r| {
            write_file(r, "a/Widget.ts", 3);
            write_file(r, "b/Widget.ts", 3);
        });
        let (status, detail) = verify_file_ref("Widget.ts:1", &[&root]);
        assert_eq!(status, SourceStatus::NotFound);
        assert!(detail.contains("ambiguous"), "{detail}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_basename_absent_is_not_found() {
        let root = temp_root_with(|r| write_file(r, "app/Other.ts", 3));
        let (status, _d) = verify_file_ref("Widget.ts:1", &[&root]);
        assert_eq!(status, SourceStatus::NotFound);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_basename_unique_but_line_out_of_bounds() {
        // Resolved by name, but the cited line is past EOF → honest OutOfBounds.
        let root = temp_root_with(|r| write_file(r, "deep/dir/Widget.ts", 2));
        let (status, _d) = verify_file_ref("Widget.ts:99", &[&root]);
        assert_eq!(status, SourceStatus::OutOfBounds, "{_d}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bare_basename_same_file_in_two_roots_is_not_ambiguous() {
        // Isolated-disc shape: roots = [worktree, main], same file in both.
        // First root wins (mirrors exact-path semantics) → not double-counted.
        let wt = temp_root_with(|r| write_file(r, "app/Widget.ts", 3));
        let main = temp_root_with(|r| write_file(r, "app/Widget.ts", 3));
        let (status, _d) = verify_file_ref("Widget.ts:1", &[&wt, &main]);
        assert_eq!(status, SourceStatus::Verified, "{_d}");
        std::fs::remove_dir_all(&wt).ok();
        std::fs::remove_dir_all(&main).ok();
    }

    #[test]
    fn inline_bare_basename_anchor_no_longer_unverified() {
        // End-to-end via analyze_roots: the exact disc d344b52b false positive.
        // A backticked bare anchor `Widget.ts:2` for a nested file used to land
        // in `unverified_count`; it now resolves green.
        let root = temp_root_with(|r| write_file(r, "app/assets/ts/Services/Widget.ts", 5));
        let report = analyze_roots("See `Widget.ts:2` for the logic.", &[&root]);
        assert_eq!(report.unverified_count, 0, "should not be a soft-amber FP");
        assert!(
            report.sources.iter().any(|s| s.status == SourceStatus::Verified),
            "the inline anchor must resolve to a Verified source: {:?}",
            report.sources
        );
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

    // ── 0.8.8 — P4 heuristic precision (false-positive fixes) ─────────

    #[test]
    fn di_opinion_conditional_not_flagged() {
        // The real-world false positive: an opinion inside a conditional.
        let t = "- La conformité DI n'est pas toujours une amélioration — si le \
                 cycle de vie est géré par le DOM, DI peut être un anti-pattern ici";
        assert_eq!(lint_assertions(t).unsourced_count, 0, "{:?}", lint_assertions(t));
    }

    #[test]
    fn genuine_unsourced_claim_still_flags() {
        // No suppressor, no anchor, a clear claim cue → must still flag.
        let t = "La fonction handleAuth est définie côté serveur dans le module auth";
        assert_eq!(lint_assertions(t).unsourced_count, 1);
    }

    #[test]
    fn hedge_accent_space_variant_suppresses() {
        // "peut être" (space + accent) must suppress like "peut-être" — the
        // accent/hyphen gap that let the DI sentence through.
        let t = "Le cache est géré localement, ça peut être un peu lent au démarrage";
        assert_eq!(lint_assertions(t).unsourced_count, 0);
    }

    #[test]
    fn conditional_cue_not_flagged() {
        let t = "Si le paramètre timeout est configuré à zéro, la requête ne coupe jamais";
        assert_eq!(lint_assertions(t).unsourced_count, 0);
    }

    #[test]
    fn conditional_si_does_not_match_inside_version() {
        // Regression: "si" must NOT match inside "version" and wrongly suppress.
        let t = "La version 7.3.6 est vulnérable à une faille de désérialisation";
        assert!(lint_assertions(t).unsourced_count >= 1, "{:?}", lint_assertions(t));
    }

    #[test]
    fn question_and_heading_not_flagged() {
        assert_eq!(
            lint_assertions("Quand tu dis que le VCL est configuré, tu veux dire quoi exactement ?").unsourced_count,
            0
        );
        assert_eq!(
            lint_assertions("### Problème 3 — où la route est définie côté backend").unsourced_count,
            0
        );
    }

    #[test]
    fn opinion_recommendation_not_flagged() {
        assert_eq!(
            lint_assertions("Le paramètre devrait être configuré côté infra plutôt qu'ici").unsourced_count,
            0
        );
        assert_eq!(
            lint_assertions("Je recommande que la route soit gérée par un middleware dédié").unsourced_count,
            0
        );
    }

    // ── 0.8.8 — P2 green badge (has_signal / verified_count) ──────────

    #[test]
    fn verified_only_report_has_signal_green() {
        let root = temp_project();
        let r = analyze("The retry logic is implemented [src: file: src/foo.rs:2].", Some(&root));
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert!(r.verified_count() >= 1, "sources: {:?}", r.sources);
        assert!(r.has_signal(), "all-verified report must carry a signal (green)");
        assert!(!r.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unchecked_only_report_has_signal_option_b() {
        // Option B (2026-05-30): a bare URL can't be machine-verified, but we
        // still SURFACE it (neutral "unverifiable" pill) — warn about everything.
        let r = analyze("See the upstream docs [src: url: https://example.com/x].", None);
        assert_eq!(r.fabricated_count, 0);
        assert_eq!(r.unsourced_count, 0);
        assert_eq!(r.verified_count(), 0, "sources: {:?}", r.sources);
        assert!(!r.sources.is_empty(), "the url source must be listed");
        assert!(r.has_signal(), "Option B: an unchecked-only report still has a signal");
        assert!(!r.is_empty());
    }

    #[test]
    fn truly_empty_report_has_no_signal() {
        let r = LintReport::empty();
        assert!(!r.has_signal());
        assert!(r.is_empty());
    }

    // ── 0.8.8 — P1 niveau-1.5 natural inline anchors ──────────────────

    #[test]
    fn inline_backticked_anchor_auto_verified_green() {
        let root = temp_project();
        // No [src:] marker — just a natural backticked `path:line` anchor.
        let r = analyze("The retry path lives in `src/foo.rs:3`, see there.", Some(&root));
        assert_eq!(r.fabricated_count, 0);
        assert!(r.verified_count() >= 1, "sources: {:?}", r.sources);
        assert!(r.has_signal());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inline_nonexistent_anchor_is_dropped_not_fabricated() {
        // A backticked path that doesn't resolve must NOT become a red
        // fabricated flag (niveau-1.5 is positive-only).
        let root = temp_project();
        let r = analyze("Logic in `src/ghost.rs:1` handles it.", Some(&root));
        assert_eq!(r.fabricated_count, 0, "sources: {:?}", r.sources);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inline_anchor_not_double_counted_with_src_marker() {
        // Same file cited BOTH as a formal [src:] marker AND inline backtick
        // must count as ONE verified source, not two (no green-chip inflation).
        let root = temp_project();
        let txt = "The retry logic [src: file: src/foo.rs:2] lives in `src/foo.rs:3`.";
        let r = analyze(txt, Some(&root));
        assert_eq!(r.verified_count(), 1, "sources: {:?}", r.sources);
        std::fs::remove_dir_all(&root).ok();
    }

    // ── 0.8.8 — finalize_lint_report (the extracted finalize seam) ────
    // ONE sequential test: `set_mode` mutates a process-global, so splitting
    // these into parallel tests would race (the known "mode-drift" risk).
    #[serial_test::serial] // mutates the process-global anti-halluc mode
    #[test]
    fn finalize_lint_report_behavior() {
        let root = temp_project();
        let worktree = temp_project();
        let main = std::env::temp_dir().join(format!("kronn_main_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&main).unwrap();

        // 1. Mode off → always None, even with a perfectly-verified citation.
        set_mode("off");
        assert!(
            finalize_lint_report("Implemented [src: file: src/foo.rs:2].", None, root.to_str().unwrap(), &[])
                .is_none(),
            "off mode must not produce a report"
        );

        // 2. Mode warn → a verified citation produces a stored GREEN report.
        set_mode("warn");
        let green = finalize_lint_report(
            "Retry logic [src: file: src/foo.rs:2].",
            None,
            root.to_str().unwrap(),
            &[],
        )
        .expect("verified report must be stored");
        assert_eq!(green.fabricated_count, 0);
        assert_eq!(green.unsourced_count, 0);
        assert!(green.verified_count() >= 1, "{:?}", green.sources);

        // 3. Plain prose, no cue, no citation → no signal → None.
        assert!(finalize_lint_report("Voilà, c'est fait.", None, "", &[]).is_none());

        // 4. Worktree root tried FIRST: file exists only in the worktree.
        let wt = finalize_lint_report(
            "See `src/foo.rs:1`.",
            Some(worktree.to_str().unwrap()),
            main.to_str().unwrap(),
            &[],
        )
        .expect("worktree-local file must verify (green)");
        assert!(wt.verified_count() >= 1, "{:?}", wt.sources);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&worktree).ok();
        std::fs::remove_dir_all(&main).ok();
        // Leave the rollout default in place for any other test reading the mode.
        set_mode("warn");
    }

    #[test]
    fn inline_anchor_requires_path_separator_and_ext() {
        // Bare backticked identifiers / calls are NOT treated as file refs.
        assert!(!looks_like_file_anchor("handleAuth()"));
        assert!(!looks_like_file_anchor("foo.rs")); // no slash
        assert!(!looks_like_file_anchor("src/foo")); // no ext
        assert!(looks_like_file_anchor("src/foo.rs"));
        assert!(looks_like_file_anchor("backend/src/lib.rs:440"));
    }

    // ── 0.8.8 — P3 citation cleaning + multi-root fallback ────────────

    #[test]
    fn verify_tolerates_backticks_quotes_punct() {
        let root = temp_project();
        for raw in ["`src/foo.rs:2`", "\"src/foo.rs\"", "'src/foo.rs'", "(src/foo.rs)", "src/foo.rs."] {
            let c = verify_source_marker(raw, Some(&root));
            assert_eq!(c.status, SourceStatus::Verified, "raw={raw} -> {c:?}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn clean_reference_strips_wrappers() {
        assert_eq!(clean_reference("`src/a.rs:1`"), "src/a.rs:1");
        assert_eq!(clean_reference("\"src/a.rs\""), "src/a.rs");
        assert_eq!(clean_reference("(src/a.rs)."), "src/a.rs");
        assert_eq!(clean_reference("src/a.rs"), "src/a.rs");
    }

    // Regression (2026-06): the dominant citation false-positive. Agents write
    // a markdown-backticked path with the line spec OUTSIDE the backticks —
    // `src/foo.rs`:3 — so the closing backtick is internal at clean time and
    // survives onto path_str after the :line split. Re-cleaning path_str fixes
    // it. (The `src/foo.rs:3` form, line INSIDE, was already handled.)
    #[test]
    fn verify_backticked_path_with_line_spec_outside() {
        let root = temp_project(); // src/foo.rs = 5 lines
        let abs = format!("`{}/src/foo.rs`:3", root.display());
        let cases: &[(&str, SourceStatus)] = &[
            ("`src/foo.rs`:3", SourceStatus::Verified),
            ("`src/foo.rs`:2-4", SourceStatus::Verified),
            ("`src/foo.rs`:6", SourceStatus::OutOfBounds), // path resolved AND bounds ran
            ("`src/foo.rs`", SourceStatus::Verified),
            ("file: `src/foo.rs`:3", SourceStatus::Verified), // file: prefix + backticks combined
            (abs.as_str(), SourceStatus::Verified),           // absolute, backticked, line outside
            ("`src/ghost.rs`:3", SourceStatus::NotFound),     // clean path, genuinely absent
        ];
        for (raw, want) in cases {
            let c = verify_source_marker(raw, Some(&root));
            assert_eq!(c.status, *want, "raw={raw:?} -> {c:?}");
        }
        // A reference that's nothing but a wrapper + line must not false-Verify
        // against the root directory.
        let empty = verify_source_marker("`:3", Some(&root));
        assert_eq!(empty.status, SourceStatus::EmptyRef, "{empty:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_segment_suffix_is_aligned() {
        assert!(path_has_segment_suffix("apps/x.php", "apps/x.php")); // equal
        assert!(path_has_segment_suffix("/r/application/apps/x.php", "apps/x.php"));
        assert!(!path_has_segment_suffix("/r/myapps/x.php", "apps/x.php")); // not segment-aligned
        assert!(!path_has_segment_suffix("/r/apps/xx.php", "apps/x.php")); // filename differs
    }

    // The dominant inline-anchor FP: project code lives under `application/`, so
    // a path cited relative to the app root (`apps/website/src/X.php`) misses at
    // the project root and must be resolved by a unique segment-aligned suffix.
    #[test]
    fn verify_resolves_app_subdir_relative_by_suffix() {
        let root = temp_project();
        std::fs::create_dir_all(root.join("application/apps/website/src")).unwrap();
        std::fs::write(root.join("application/apps/website/src/Justin.php"), "a\nb\nc\n").unwrap();
        let c = verify_source_marker("apps/website/src/Justin.php:2", Some(&root));
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        assert!(c.detail.contains("unique path suffix"), "{c:?}");
        // Bounds still enforced on the resolved file.
        let oob = verify_source_marker("apps/website/src/Justin.php:9", Some(&root));
        assert_eq!(oob.status, SourceStatus::OutOfBounds, "{oob:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_suffix_ambiguous_stays_not_found() {
        let root = temp_project();
        std::fs::create_dir_all(root.join("a/dir")).unwrap();
        std::fs::create_dir_all(root.join("b/dir")).unwrap();
        std::fs::write(root.join("a/dir/Same.php"), "x\n").unwrap();
        std::fs::write(root.join("b/dir/Same.php"), "x\n").unwrap();
        let c = verify_source_marker("dir/Same.php", Some(&root));
        assert_eq!(c.status, SourceStatus::NotFound, "ambiguous suffix must not verify: {c:?}");
        assert!(c.detail.contains("ambiguous"), "{c:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn verify_cross_repo_path_via_linked_root() {
        // A sibling repo passed as a second root: a repo-name-prefixed citation
        // `front_apollo/plugins/...` resolves against the linked root.
        let main = temp_project();
        let parent = std::env::temp_dir().join(format!("kronn_apollo_{}", uuid::Uuid::new_v4()));
        let apollo = parent.join("front_apollo");
        std::fs::create_dir_all(apollo.join("plugins/timeline")).unwrap();
        std::fs::write(apollo.join("plugins/timeline/index.js"), "a\nb\n").unwrap();
        let c = verify_source_marker_roots(
            "front_apollo/plugins/timeline/index.js:1",
            &[main.as_path(), apollo.as_path()],
        );
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        // A genuinely absent cross-repo path still fails.
        let ghost = verify_source_marker_roots(
            "front_apollo/plugins/ghost/nope.js",
            &[main.as_path(), apollo.as_path()],
        );
        assert_eq!(ghost.status, SourceStatus::NotFound, "{ghost:?}");
        std::fs::remove_dir_all(&main).ok();
        std::fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn verify_falls_back_to_second_root() {
        // File exists only in root #1 (worktree), not root #2 (main checkout).
        let worktree = temp_project();
        let main = std::env::temp_dir().join(format!("kronn_main_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&main).unwrap(); // empty: no src/foo.rs
        let c = verify_source_marker_roots(
            "src/foo.rs:1",
            &[worktree.as_path(), main.as_path()],
        );
        assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&worktree).ok();
        std::fs::remove_dir_all(&main).ok();
    }

    #[test]
    fn verify_traversal_still_jailed_multi_root() {
        let a = temp_project();
        let b = temp_project();
        let c = verify_source_marker_roots("../../../../etc/passwd:1", &[a.as_path(), b.as_path()]);
        assert_ne!(c.status, SourceStatus::Verified, "{c:?}");
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    #[test]
    fn verify_missing_in_all_roots_is_not_found() {
        let a = temp_project();
        let b = temp_project();
        let c = verify_source_marker_roots("src/ghost.rs:1", &[a.as_path(), b.as_path()]);
        assert_eq!(c.status, SourceStatus::NotFound, "{c:?}");
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    // ── 0.8.8 — P5 PREAMBLE wording invariants ────────────────────────

    #[test]
    fn preamble_blesses_natural_anchors_and_opinion_convention() {
        // The new wording must tell agents natural backticked paths count …
        assert!(PREAMBLE.contains("backticked"), "PREAMBLE must bless natural anchors");
        // … and give them a way to mark opinions/guesses.
        assert!(
            PREAMBLE.contains("recommendation") || PREAMBLE.contains("recommande"),
            "PREAMBLE must describe the opinion-marking convention"
        );
    }

    // ── corpus precision guard ────────────────────────────────────────
    //
    // 0.8.8 — a CORPUS-BASED precision pin for the niveau-0 prose
    // heuristic (`lint_assertions`). Distinct from the JSONL fixture
    // gate above (`corpus_false_positive_rate_under_5_percent`): this is
    // an inline, hand-curated, mixed FR/EN/ES set split into two labelled
    // arrays so a regression NAMES the exact culprit sentence.
    //
    //   - SHOULD_FLAG   : genuine confident technical assertions about
    //     THIS codebase — claim cue, NO anchor, NO hedge. Each MUST
    //     produce `unsourced_count >= 1`. Pins RECALL.
    //   - MUST_NOT_FLAG : the known false-positive classes (opinion,
    //     conditional, question, heading, imperative bullet, hedge,
    //     anchored, third-party disclaimer, bare semver / proper noun).
    //     Each MUST produce `unsourced_count == 0`. Pins PRECISION.
    //
    // If a future heuristic change silently flips precision or recall,
    // one of the two tests below fails and prints the offending sentence.

    /// Genuine confident technical claims that MUST flag (recall pin).
    /// All anchorless, hedgeless, cued. Mixed FR / EN / ES.
    const SHOULD_FLAG: &[&str] = &[
        // ── FR — code / config ──
        "La fonction handleAuth est définie dans le module auth côté serveur",
        "La route de déconnexion est gérée par le middleware de session",
        "Le paramètre de timeout est configuré à trente secondes pour tous",
        "La table des sessions est stockée dans la base principale du backend",
        "La méthode de cache renvoie systématiquement la valeur expirée ici",
        // ── EN — code / config ──
        "The handleAuth function is defined in the server-side auth module",
        "The endpoint returns a 500 error when given a null input payload",
        "The retry logic is implemented with an exponential backoff loop here",
        "The session token is stored in the primary database table by default",
        "The cache invalidation flag is set in the startup configuration block",
        // ── ES — security / CVE (ES code/config verbs like "está
        //   configurado" / "está definido" have NO cue in CLAIM_CUES — a
        //   documented recall gap, see HEURISTIC WEAKNESSES below. The
        //   cross-lingual cues that DO fire on Spanish prose are the
        //   tokens shared verbatim: "CVE-" and bare-frame English verbs.
        //   This ES claim flags via the language-agnostic "cve-" cue.) ──
        "CVE-2025-64500 afecta a la versión 7.3.6 del framework según el aviso",
        // ── FR — security / CVE / version ──
        "La version 7.3.6 du framework est vulnérable à une faille PATH_INFO",
        "La faille de désérialisation est corrigé dans la release 7.3.7 publiée",
        "Les versions affectées vont de la 6.0 jusqu'à la 7.3.6 inclusivement",
        "La dernière version est la 8.2.0 sortie ce matin sur le dépôt amont",
        // ── EN — security / CVE / version ──
        "CVE-2025-64500 is a high-severity flaw in the request handling layer",
        "The serialization bug is fixed in version 7.3.7 of the upstream library",
        "The deserialization gadget is vulnerable to a remote code execution path",
    ];

    /// False-positive classes that MUST NOT flag (precision pin).
    /// Mixed FR / EN / ES, one sentence per failure mode.
    const MUST_NOT_FLAG: &[&str] = &[
        // ── opinions / recommendations ──
        "Les services dédiés sont préférables pour ce genre de découpage métier",
        "On devrait extraire ça dans un module séparé pour clarifier la logique",
        "Je recommande que la route soit gérée par un middleware dédié et testé",
        "À mon avis le paramètre devrait être configuré côté infra plutôt qu'ici",
        "I recommend the endpoint should be handled by a dedicated gateway layer",
        "It would be better if the cache were configured with a shorter window",
        // ── conditionals / hypotheticals ──
        "Si le cache est configuré ainsi, la fonction renvoie une valeur périmée",
        "Quand le flag est activé, la route est gérée par le nouveau middleware",
        "When the feature flag is on, the endpoint returns the cached response",
        "Lorsque le timeout est configuré à zéro, la requête ne coupe jamais ici",
        "If the parameter is set in the override block, the default is ignored",
        // ── questions ──
        // NOTE: a question is reliably suppressed ONLY when it has no
        // claim cue, OR a conditional opener precedes the cue. A bare
        // interrogative ("Où … est configuré ?", "Est-ce que … est
        // définie ?") is NOT suppressed by `is_question` — see HEURISTIC
        // WEAKNESS #1 below — so those two FR forms are parked in the
        // documented BORDERLINE list, not asserted here.
        "Where is the retry logic implemented in the current backend codebase ?",
        // ── markdown headings ──
        "### Problème 3 — où la route de déconnexion est définie côté backend",
        "## La fonction handleAuth est définie où exactement dans ce dépôt **",
        // ── imperative bullets ──
        "- Spécifier l'endpoint exact que la passerelle doit appeler au démarrage",
        "- Vérifier que le paramètre de timeout est configuré correctement partout",
        "- Add the missing endpoint and configure the retry flag in the manifest",
        // ── hedged ──
        "Je pense que la fonction handleAuth est définie dans le module auth ici",
        "Peut-être que l'endpoint renvoie une 500 sur une entrée nulle, à vérifier",
        "Le cache est géré localement, ça peut être un peu lent au tout démarrage",
        "The endpoint probably returns a 500 on null input, but let me check first",
        // ── anchored (backtick path / [src:] / URL) ──
        "The retry logic is implemented in `backend/src/core/retry.rs` for callers",
        "La fonction de cache se trouve dans `backend/src/core/cache.rs` au démarrage",
        "The default window is thirty minutes [src: file: src/config.rs:12] exactly",
        "The endpoint returns paginated JSON per https://api.example.com/docs spec",
        // ── third-party / runtime disclaimers ──
        // The FR "X est géré par l'équipe infra" form carries the "est
        // géré" cue and reads as a confident attribution claim, so the
        // linter flags it — arguably correct (it IS an unsourced claim
        // about ownership). Parked in BORDERLINE, not asserted here. The
        // EN disclaimer below carries no cue → reliably silent.
        "That part is owned by the platform team and lives outside this repository",
        // ── bare semver / proper nouns (no claim cue) ──
        "Version 1.2.3 sort demain selon le calendrier de release prévu par avance",
        "L'équipe d'Élodie a configuré le déploiement chez le fournisseur cloud hier",
    ];

    /// Genuinely-ambiguous sentences whose CURRENT linter verdict is a
    /// known, documented edge — NOT asserted as pass/fail here, kept so
    /// the corpus stays honest (they were NOT deleted to make a test
    /// green). See HEURISTIC WEAKNESSES below. Each tuple is
    /// `(sentence, currently_flags)`.
    const BORDERLINE: &[(&str, bool)] = &[
        // WEAKNESS #1 — FIXED 2026-05-29. Bare interrogatives used to be
        // flagged: `split_sentences` consumed the terminal `?` as a
        // boundary delimiter and DROPPED it, so `is_question` (which checks
        // `ends_with('?')`) never fired on a trailing question. The fix
        // retains a terminating `?`/`!` on the flushed sentence, so a cued
        // question (incl. the French "… ?" with the standard space-before-?)
        // is now correctly suppressed. These two no longer flag:
        ("Où est-ce que le paramètre de timeout est configuré dans ce projet ?", false),
        ("Est-ce que la fonction handleAuth est définie côté serveur ou client ?", false),
        // WEAKNESS #2 — an ownership attribution ("X est géré par l'équipe
        // Y") carries the "est géré" cue and is flagged. Defensible as a
        // genuine unsourced claim, but it reads to a human as a scoping
        // disclaimer, so it is borderline rather than a clean FP:
        ("Ce point est géré par l'équipe infra, pas dans ce repo applicatif côté nous", true),
    ];

    // ── HEURISTIC WEAKNESSES (found while building this corpus) ────────
    //
    // #1  Bare-interrogative blind spot (precision) — FIXED 2026-05-29.
    //     `is_question` WAS effectively unreachable for a question that
    //     ENDS the text: `split_sentences` stripped the terminal `?` before
    //     `is_question` ran, so a cued question with no conditional opener
    //     ("Où … est configuré ?") was wrongly flagged. The fix retains the
    //     terminating `?`/`!` on the flushed sentence; the BORDERLINE tuples
    //     above now record the corrected (non-flagging) verdict.
    //
    // #2  No Spanish cue coverage (recall). CLAIM_CUES has zero ES verbs:
    //     "está configurado / definido / implementado / almacenado" don't
    //     match anything. Spanish technical claims only flag when they
    //     reuse a language-agnostic token ("CVE-", a backticked path, an
    //     English frame). Genuine ES prose claims pass through unflagged.
    //
    // #3  Ownership-attribution overlap (precision). "est géré par …"
    //     fires on scoping disclaimers that a human reads as "not my
    //     problem", not as a checkable assertion. Borderline by design.

    #[test]
    fn borderline_cases_documented_not_silently_dropped() {
        // We don't assert these are "right" — only that their CURRENT
        // verdict matches what the comment block above claims, so the doc
        // can't drift away from the code without this test catching it.
        for &(s, expected_flags) in BORDERLINE {
            let flags = lint_assertions(s).unsourced_count >= 1;
            assert_eq!(
                flags, expected_flags,
                "BORDERLINE verdict drifted for {s:?}: doc says flags={expected_flags}, got flags={flags}. \
                 Update the HEURISTIC WEAKNESSES note (and the heuristic if this is a real fix).",
            );
        }
    }

    #[test]
    fn corpus_should_flag_all_genuine_claims() {
        // RECALL pin: every genuine confident claim must flag. On failure
        // we collect the culprits so a regression names exactly which
        // sentence the heuristic stopped catching.
        let mut missed: Vec<&str> = Vec::new();
        for &s in SHOULD_FLAG {
            let r = lint_assertions(s);
            if r.unsourced_count < 1 {
                missed.push(s);
            }
        }
        assert!(
            missed.is_empty(),
            "{}/{} genuine claims were NOT flagged (recall regression):\n  - {}",
            missed.len(),
            SHOULD_FLAG.len(),
            missed.join("\n  - "),
        );
    }

    #[test]
    fn corpus_false_positive_rate_under_threshold() {
        // PRECISION pin: every false-positive-class sentence must stay at
        // unsourced_count == 0. Target FP count is 0; we keep a ≤ 5%
        // tolerance band so a single genuinely-borderline case doesn't
        // hard-fail the suite, but we ALWAYS print the offenders so the
        // regression is visible and a human can decide.
        let mut offenders: Vec<String> = Vec::new();
        for &s in MUST_NOT_FLAG {
            let r = lint_assertions(s);
            if r.unsourced_count != 0 {
                offenders.push(format!(
                    "[cue={}] \"{}\"",
                    r.flagged_spans.first().map(|f| f.reason.as_str()).unwrap_or("?"),
                    s,
                ));
            }
        }
        let total = MUST_NOT_FLAG.len();
        let fp = offenders.len();
        let fp_rate = fp as f64 / total as f64;
        assert!(
            fp_rate <= 0.05,
            "FP rate {:.1}% exceeds 5% gate ({fp}/{total}) — precision regression:\n  {}",
            fp_rate * 100.0,
            offenders.join("\n  "),
        );
        // Document the ideal: we currently expect ZERO false positives.
        // If this ever trips while the rate gate still passes, a borderline
        // case slipped in — move it to a documented borderline list above
        // rather than silently tolerating drift.
        assert_eq!(
            fp, 0,
            "expected zero false positives on the curated corpus, got {fp}:\n  {}",
            offenders.join("\n  "),
        );
    }

    // ════════════════════════════════════════════════════════════════════
    // EXHAUSTIVE MATRIX COVERAGE (added 2026-05-29)
    // Grouped in sub-modules; each re-imports the parent scope (`super::*`)
    // so `temp_project()`, the public API, and the private helpers are all
    // in scope. No duplication of the baseline tests above — these widen the
    // matrix on the gaps called out in the QA roadmap.
    // ════════════════════════════════════════════════════════════════════

    // ── 1. classify_source — full prefix matrix ──────────────────────────
    mod classify_matrix {
        use super::*;

        #[test]
        fn every_typed_prefix_with_colon() {
            // (input, expected kind, expected stripped reference)
            let cases: &[(&str, SourceKind, &str)] = &[
                ("file: src/a.rs:10", SourceKind::File, "src/a.rs:10"),
                // A `url:`-prefixed value that contains an actual URL is caught
                // by the bare-URL substring check FIRST (before prefix
                // stripping), so the whole string is kept as the reference.
                // This is fine — Url is Unchecked, the reference is display-only.
                ("url: https://x.io/p", SourceKind::Url, "url: https://x.io/p"),
                // A `url:`-prefixed value WITHOUT a scheme falls through to the
                // keyword branch and DOES get stripped.
                ("url: docs/guide", SourceKind::Url, "docs/guide"),
                ("user: 2026-05-25: standup note", SourceKind::User, "2026-05-25: standup note"),
                ("api: GET /widgets#3", SourceKind::Api, "GET /widgets#3"),
                ("commit: deadbeef99", SourceKind::Commit, "deadbeef99"),
                ("code-comment: src/a.rs:4", SourceKind::CodeComment, "src/a.rs:4"),
                ("inferred: from the pattern", SourceKind::Inferred, "from the pattern"),
                ("hypothesis: maybe a cache", SourceKind::Hypothesis, "maybe a cache"),
                ("training-data: i recall", SourceKind::TrainingData, "i recall"),
            ];
            for (raw, kind, reference) in cases {
                let (k, r) = classify_source(raw);
                assert_eq!(k, *kind, "kind for {raw:?}");
                assert_eq!(r, *reference, "reference for {raw:?}");
            }
        }

        #[test]
        fn typed_prefix_with_whitespace_no_colon() {
            // `match_type_keyword` accepts `<keyword><space>` as a boundary too.
            assert_eq!(classify_source("file src/a.rs").0, SourceKind::File);
            assert_eq!(classify_source("commit deadbeef").0, SourceKind::Commit);
            assert_eq!(classify_source("inferred the design").0, SourceKind::Inferred);
            // …and the bare keyword on its own → empty remainder.
            let (k, r) = classify_source("hypothesis");
            assert_eq!(k, SourceKind::Hypothesis);
            assert_eq!(r, "");
        }

        #[test]
        fn bare_url_http_and_https_no_prefix() {
            assert_eq!(classify_source("https://example.com/a").0, SourceKind::Url);
            assert_eq!(classify_source("http://example.com/a").0, SourceKind::Url);
            // URL embedded after some prose still detected (substring match).
            assert_eq!(classify_source("see https://example.com here").0, SourceKind::Url);
        }

        #[test]
        fn user_confirmed_phrase_both_forms() {
            assert_eq!(classify_source("user-confirmed 2026-05-25").0, SourceKind::User);
            assert_eq!(classify_source("user confirmed by the lead").0, SourceKind::User);
            // Case-insensitive.
            assert_eq!(classify_source("User-Confirmed yesterday").0, SourceKind::User);
        }

        #[test]
        fn token_boundary_real_filenames_are_file_not_type() {
            // A file whose NAME starts with a type keyword must stay File —
            // the keyword-boundary check is the guard. Exhaustive sweep over
            // every prefix that has a same-named-file collision risk.
            let collisions = [
                "user_service.rs:10",
                "userland.ts",
                "api_client.rs:1",
                "apiconfig.json",
                "commit_log.rs",
                "committed.txt",
                "inferred_types.ts",
                "filesystem.rs",
                "urls.txt",
                "hypotheses.md",
            ];
            for path in collisions {
                assert_eq!(
                    classify_source(path).0,
                    SourceKind::File,
                    "{path} must be File (token boundary), not a typed source",
                );
            }
        }

        #[test]
        fn unknown_bare_token_falls_back_to_file() {
            // No known prefix, no URL, no user phrase → File fallback, whole
            // string kept as the reference.
            let (k, r) = classify_source("just/some/path.rs:3");
            assert_eq!(k, SourceKind::File);
            assert_eq!(r, "just/some/path.rs:3");
        }

        #[test]
        fn training_data_wins_over_substring_collisions() {
            // `training-data` is checked first; it must not be shadowed by a
            // later substring match.
            assert_eq!(classify_source("training-data").0, SourceKind::TrainingData);
        }

        #[test]
        fn api_keyword_does_not_swallow_url_prefix() {
            // `url:` and `api:` are distinct keywords. A `url:` whose value
            // happens to contain "api" stays Url.
            assert_eq!(classify_source("url: https://api.example.com").0, SourceKind::Url);
        }
    }

    // ── 2. verify_source_marker — full status matrix ─────────────────────
    mod verify_status_matrix {
        use super::*;

        #[test]
        fn verified_exists_single_line_in_bounds() {
            let root = temp_project();
            assert_eq!(verify_source_marker("src/foo.rs:1", Some(&root)).status, SourceStatus::Verified);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn verified_range_fully_in_bounds() {
            let root = temp_project();
            assert_eq!(verify_source_marker("src/foo.rs:1-5", Some(&root)).status, SourceStatus::Verified);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn not_found_relative_missing_file() {
            let root = temp_project();
            assert_eq!(verify_source_marker("src/missing.rs", Some(&root)).status, SourceStatus::NotFound);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn out_of_bounds_single_line_and_range() {
            let root = temp_project(); // foo.rs has 5 lines
            assert_eq!(verify_source_marker("src/foo.rs:99", Some(&root)).status, SourceStatus::OutOfBounds);
            assert_eq!(verify_source_marker("src/foo.rs:3-50", Some(&root)).status, SourceStatus::OutOfBounds);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn empty_ref_variants() {
            let root = temp_project();
            for raw in ["file:", "file: ", "file:   "] {
                assert_eq!(
                    verify_source_marker(raw, Some(&root)).status,
                    SourceStatus::EmptyRef,
                    "raw={raw:?}",
                );
            }
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn outside_project_relative_escape() {
            let root = temp_project();
            assert_eq!(
                verify_source_marker("../escape.rs", Some(&root)).status,
                SourceStatus::OutsideProject,
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unchecked_soft_and_network_tiers() {
            let root = temp_project();
            // No root → Unchecked.
            assert_eq!(verify_source_marker("src/foo.rs:1", None).status, SourceStatus::Unchecked);
            // URL / api / commit / user / inferred / hypothesis are never
            // file-verified — Unchecked regardless of root.
            let unchecked = [
                "url: https://x.io",
                "api: GET /x",
                "commit: abc1234",
                "user-confirmed 2026-01-01",
                "inferred: a guess",
                "hypothesis: maybe",
            ];
            for raw in unchecked {
                let c = verify_source_marker(raw, Some(&root));
                assert_eq!(c.status, SourceStatus::Unchecked, "raw={raw}");
                assert!(!c.status.is_fabricated(), "raw={raw} must not be fabricated");
            }
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn rejected_training_data_is_fabricated() {
            let root = temp_project();
            // Even pointing at a real file, training-data is Rejected.
            let c = verify_source_marker("training-data: src/foo.rs:1", Some(&root));
            assert_eq!(c.status, SourceStatus::Rejected);
            assert!(c.status.is_fabricated());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn code_comment_verified_but_low_trust_detail() {
            let root = temp_project();
            let c = verify_source_marker("code-comment: src/foo.rs:2", Some(&root));
            assert_eq!(c.status, SourceStatus::Verified);
            assert!(
                c.detail.to_lowercase().contains("not authoritative")
                    || c.detail.to_lowercase().contains("code comment"),
                "low-trust detail expected: {}",
                c.detail,
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn absolute_path_existence_only_no_jail() {
            let root = temp_project();
            // Absolute path is checked for existence only — no project jail.
            // foo.rs lives under root; cite it by absolute path.
            let abs = root.join("src/foo.rs");
            let c = verify_source_marker(&format!("{}", abs.display()), Some(&root));
            assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn is_fabricated_covers_exactly_the_hard_failures() {
            // Pin the exact membership of `is_fabricated` — drives the red pill.
            assert!(SourceStatus::NotFound.is_fabricated());
            assert!(SourceStatus::OutOfBounds.is_fabricated());
            assert!(SourceStatus::EmptyRef.is_fabricated());
            assert!(SourceStatus::OutsideProject.is_fabricated());
            assert!(SourceStatus::Rejected.is_fabricated());
            // …and NOT the honest-uncertainty ones.
            assert!(!SourceStatus::Verified.is_fabricated());
            assert!(!SourceStatus::Unchecked.is_fabricated());
        }
    }

    // ── 3. clean_reference — wrapper-stripping edge cases ─────────────────
    mod clean_reference_edges {
        use super::*;

        #[test]
        fn strips_every_single_wrapper_kind() {
            assert_eq!(clean_reference("`a/b.rs`"), "a/b.rs");
            assert_eq!(clean_reference("\"a/b.rs\""), "a/b.rs");
            assert_eq!(clean_reference("'a/b.rs'"), "a/b.rs");
            assert_eq!(clean_reference("<a/b.rs>"), "a/b.rs");
            assert_eq!(clean_reference("(a/b.rs)"), "a/b.rs");
        }

        #[test]
        fn strips_trailing_sentence_punctuation() {
            assert_eq!(clean_reference("a/b.rs."), "a/b.rs");
            assert_eq!(clean_reference("a/b.rs,"), "a/b.rs");
            assert_eq!(clean_reference("a/b.rs;"), "a/b.rs");
        }

        #[test]
        fn strips_nested_wrappers() {
            assert_eq!(clean_reference("`\"a/b.rs\"`"), "a/b.rs");
            assert_eq!(clean_reference("(`a/b.rs`)."), "a/b.rs");
            assert_eq!(clean_reference("<`a/b.rs:42`>,"), "a/b.rs:42");
        }

        #[test]
        fn leading_dot_slash_is_preserved_as_relative() {
            // `clean_reference` only strips wrappers/punct — it must NOT mangle
            // a `./` relative prefix into something else.
            assert_eq!(clean_reference("./src/a.rs"), "./src/a.rs");
            assert_eq!(clean_reference("`./src/a.rs`"), "./src/a.rs");
        }

        #[test]
        fn backtick_wrapped_escape_stays_an_escape_not_unwrapped_into_safety() {
            // A `../` smuggled inside backticks must STILL read as `../…` after
            // cleaning — clean_reference strips the ticks but never resolves the
            // path, so the downstream jail still sees the escape.
            assert_eq!(clean_reference("`../../etc/passwd`"), "../../etc/passwd");
            let root = temp_project();
            let c = verify_source_marker("`../../etc/passwd:1`", Some(&root));
            assert_eq!(
                c.status,
                SourceStatus::OutsideProject,
                "backtick-wrapped escape must remain jailed: {c:?}",
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn path_with_parens_in_name_round_trips_through_verify() {
            // clean_reference strips OUTER parens greedily, so we verify the
            // realistic case: a backtick-wrapped path verifies without paren
            // collision.
            let root = temp_project();
            std::fs::write(root.join("src/weird.rs"), "x\n").unwrap();
            let c = verify_source_marker("`src/weird.rs`", Some(&root));
            assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn idempotent_on_clean_input() {
            assert_eq!(clean_reference("src/a.rs:1-9"), "src/a.rs:1-9");
        }
    }

    // ── 4. multi-root resolution matrix ──────────────────────────────────
    mod multi_root_matrix {
        use super::*;

        fn empty_root() -> PathBuf {
            let d = std::env::temp_dir().join(format!("kronn_empty_{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        #[test]
        fn found_in_root1_only() {
            let r1 = temp_project();
            let r2 = empty_root();
            let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
            assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
            std::fs::remove_dir_all(&r1).ok();
            std::fs::remove_dir_all(&r2).ok();
        }

        #[test]
        fn found_in_root2_only() {
            let r1 = empty_root();
            let r2 = temp_project();
            let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
            assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
            std::fs::remove_dir_all(&r1).ok();
            std::fs::remove_dir_all(&r2).ok();
        }

        #[test]
        fn found_in_neither_is_not_found() {
            let r1 = empty_root();
            let r2 = empty_root();
            let c = verify_source_marker_roots("src/foo.rs:1", &[r1.as_path(), r2.as_path()]);
            assert_eq!(c.status, SourceStatus::NotFound, "{c:?}");
            std::fs::remove_dir_all(&r1).ok();
            std::fs::remove_dir_all(&r2).ok();
        }

        #[test]
        fn found_in_both_resolves_verified() {
            let r1 = temp_project();
            let r2 = temp_project();
            let c = verify_source_marker_roots("src/foo.rs:3", &[r1.as_path(), r2.as_path()]);
            assert_eq!(c.status, SourceStatus::Verified, "{c:?}");
            std::fs::remove_dir_all(&r1).ok();
            std::fs::remove_dir_all(&r2).ok();
        }

        #[test]
        fn escape_in_all_roots_is_outside_project() {
            let r1 = temp_project();
            let r2 = temp_project();
            let c = verify_source_marker_roots("../../../../etc/passwd", &[r1.as_path(), r2.as_path()]);
            assert_eq!(c.status, SourceStatus::OutsideProject, "{c:?}");
            std::fs::remove_dir_all(&r1).ok();
            std::fs::remove_dir_all(&r2).ok();
        }

        #[test]
        fn empty_roots_slice_is_unchecked() {
            let c = verify_source_marker_roots("src/foo.rs:1", &[]);
            assert_eq!(c.status, SourceStatus::Unchecked, "{c:?}");
        }
    }

    // ── 5. inline file anchors — looks_like / extract ────────────────────
    mod inline_anchor_matrix {
        use super::*;

        #[test]
        fn every_supported_extension_is_an_anchor() {
            const EXTS: &[&str] = &[
                "rs", "ts", "tsx", "js", "jsx", "py", "toml", "json", "sql",
                "md", "yml", "yaml", "css", "html", "sh", "php", "go", "java",
                "rb", "vue", "svelte", "c", "cpp", "h",
            ];
            for ext in EXTS {
                let path = format!("src/dir/file.{ext}");
                assert!(looks_like_file_anchor(&path), "{path} should be an anchor");
            }
        }

        #[test]
        fn rejects_no_slash() {
            assert!(!looks_like_file_anchor("foo.rs"));
            assert!(!looks_like_file_anchor("Cargo.toml"));
        }

        #[test]
        fn rejects_no_extension() {
            assert!(!looks_like_file_anchor("src/foo"));
            assert!(!looks_like_file_anchor("a/b/c"));
        }

        #[test]
        fn accepts_root_level_file_with_explicit_line() {
            // Recall fix (live test, DOCROMS_WEB): a root-level file cited WITH
            // a line is a real file anchor even without a path separator —
            // `composer.json:1` resolved on disk but was previously dropped.
            assert!(looks_like_file_anchor("composer.json:1"));
            assert!(looks_like_file_anchor("Cargo.toml:5"));
            assert!(looks_like_file_anchor("README.md:3"));
            // … but the SAME filename with NO line + NO slash stays out, so
            // prose like `node.js` / `package.json` (mentioned, not cited) is
            // not mistaken for a citation.
            assert!(!looks_like_file_anchor("composer.json"));
            assert!(!looks_like_file_anchor("node.js"));
        }

        #[test]
        fn rejects_whitespace_and_empty() {
            assert!(!looks_like_file_anchor(""));
            assert!(!looks_like_file_anchor("src/foo bar.rs"));
        }

        #[test]
        fn accepts_anchor_with_line_and_range() {
            assert!(looks_like_file_anchor("src/foo.rs:42"));
            assert!(looks_like_file_anchor("a/b/c.ts:10-20"));
        }

        #[test]
        fn accepts_web_template_and_i18n_extensions() {
            // Symfony / web-project files. A 4-persona forensic re-pass found
            // EVERY `.twig` / `.xlf` citation went unverified because these were
            // missing from the allowlist (the central files of a Symfony repo).
            assert!(looks_like_file_anchor("templates/pages/projets.html.twig:82"));
            assert!(looks_like_file_anchor("templates/menu/header.html.twig"));
            assert!(looks_like_file_anchor("translations/messages.it.xlf"));
            assert!(looks_like_file_anchor("assets/styles/app.scss:10"));
            // Double extension needs no special case: ends_with(".twig").
            assert!(looks_like_file_anchor("a/b.html.twig"));
            // Precision kept: a bare root filename without slash OR :line stays
            // out (ambiguous), and bare extensionless prose never matches.
            assert!(!looks_like_file_anchor("twig"));
            assert!(!looks_like_file_anchor("projets.html.twig"));
        }

        #[test]
        fn ext_lists_are_unified_via_shared_const() {
            // contains_code_anchor (niveau-0) and looks_like_file_anchor
            // (niveau-1.5) now share SOURCE_EXTS so they cannot drift — a real
            // divergence the forensic re-pass found (.php lived in one list only).
            for ext in [".twig", ".xlf", ".scss", ".php", ".vue", ".rs"] {
                assert!(SOURCE_EXTS.contains(&ext), "SOURCE_EXTS must carry {ext}");
                // A path token with this ext + a separator reads as a niveau-0
                // anchor (so the citing sentence isn't flagged unsourced).
                let s = format!("voir `dir/file{ext}` pour les détails");
                assert!(contains_code_anchor(&s), "contains_code_anchor missed {ext}");
            }
        }

        #[test]
        fn analyze_verifies_real_twig_anchor_green() {
            // Clarisse cited `templates/pages/projets.html.twig:82` (a real file,
            // line 82 exact on disk). Before the fix: dropped. Now: green.
            let root = temp_project();
            std::fs::create_dir_all(root.join("templates/pages")).unwrap();
            std::fs::write(root.join("templates/pages/projets.html.twig"), "l1\nl2\nl3\n").unwrap();
            let r = analyze(
                "La section va dans `templates/pages/projets.html.twig:2`.",
                Some(&root),
            );
            assert_eq!(r.verified_count(), 1, "twig anchor must verify green: {:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn analyze_surfaces_out_of_bounds_twig_line_honestly() {
            // Lea cited `home/projets.html.twig:97-116` on a 24-line file. Now
            // that `.twig` resolves, an inline anchor with an out-of-bounds line
            // is surfaced as soft-amber "couldn't verify" (NOT silently dropped,
            // NOT red — inline anchors are lower-confidence than formal [src:]).
            let root = temp_project();
            std::fs::create_dir_all(root.join("templates/home")).unwrap();
            std::fs::write(root.join("templates/home/projets.html.twig"), "a\nb\nc\n").unwrap();
            let r = analyze("Voir `templates/home/projets.html.twig:97-116`.", Some(&root));
            assert_eq!(r.verified_count(), 0, "out-of-bounds line must NOT be green");
            assert_eq!(r.unverified_count, 1, "must surface as soft-amber: {:?}", r.sources);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn extract_skips_fenced_code() {
            let txt = "See `src/a.rs`.\n```\n`src/fenced.rs`\n```\nEnd.";
            let anchors = extract_inline_file_anchors(txt);
            assert_eq!(anchors, vec!["src/a.rs"], "fenced anchor must be skipped");
        }

        #[test]
        fn extract_multiple_anchors_in_one_text() {
            let txt = "Both `src/a.rs:1` and `lib/b.ts` are relevant here.";
            let anchors = extract_inline_file_anchors(txt);
            assert_eq!(anchors, vec!["src/a.rs:1", "lib/b.ts"]);
        }

        #[test]
        fn extract_ignores_non_anchor_backticks() {
            // Backticked identifiers / calls / non-path tokens are NOT anchors.
            let txt = "Call `handleAuth()` then read `Cargo.toml` and `foo`.";
            assert!(extract_inline_file_anchors(txt).is_empty());
        }

        #[test]
        fn dedup_inline_anchor_against_src_marker_same_path() {
            // Same path via [src:] AND inline backtick → counted once.
            let root = temp_project();
            let txt = "Logic [src: file: src/foo.rs:2] sits in `src/foo.rs:4`.";
            let r = analyze(txt, Some(&root));
            assert_eq!(r.verified_count(), 1, "must not double-count: {:?}", r.sources);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn inline_anchor_with_line_range_auto_verified() {
            let root = temp_project(); // foo.rs = 5 lines
            let r = analyze("Range `src/foo.rs:2-4` is the hot path.", Some(&root));
            assert!(r.verified_count() >= 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            std::fs::remove_dir_all(&root).ok();
        }
    }

    // ── 6. lint_assertions — FR/EN/ES precision matrix ───────────────────
    mod lint_precision_matrix {
        use super::*;

        // ----- SHOULD flag: genuine claim, cue present, no anchor/hedge -----

        #[test]
        fn flags_genuine_en_claims() {
            let claims = [
                "The auth token is stored in the session table for every request.",
                "The endpoint returns a paginated list of widgets for the caller.",
                "The migration is defined for the whole production fleet here.",
            ];
            for c in claims {
                assert_eq!(lint_assertions(c).unsourced_count, 1, "should flag: {c}");
            }
        }

        #[test]
        fn flags_genuine_fr_claims() {
            let claims = [
                "La route principale est gérée par un middleware dédié côté serveur.",
                "Le paramètre de timeout est configuré globalement pour tout le cluster.",
                "La table des sessions se trouve dans le schéma applicatif principal.",
            ];
            for c in claims {
                assert_eq!(lint_assertions(c).unsourced_count, 1, "should flag: {c}");
            }
        }

        #[test]
        fn es_genuine_claims_now_flag_recall_gap_closed() {
            // 0.8.8 — the ES recall gap is CLOSED: native Spanish claim cues
            // ("la función", "se encuentra", "está definido"…) now exist, so a
            // pure-Spanish code claim flags WITHOUT relying on a shared token.
            let pure_es = "La función de autenticación se encuentra en el servidor principal del sistema.";
            assert!(
                lint_assertions(pure_es).unsourced_count >= 1,
                "pure Spanish code claim must now flag (gap closed): {:?}",
                lint_assertions(pure_es),
            );
            let es_cve = "El sistema es vulnerable: CVE-2025-12345 afecta al parser principal del backend.";
            assert!(lint_assertions(es_cve).unsourced_count >= 1, "ES CVE claim must flag");
            // Precision kept: an ES OPINION must still NOT flag.
            let es_opinion = "En mi opinión, sería preferible mover esa lógica a un servicio dedicado y reutilizable.";
            assert_eq!(
                lint_assertions(es_opinion).unsourced_count, 0,
                "ES opinion must not flag: {:?}", lint_assertions(es_opinion),
            );
            // And an ES hedge suppresses.
            let es_hedge = "Creo que la función de login está definida en el controlador, pero no estoy seguro.";
            assert_eq!(lint_assertions(es_hedge).unsourced_count, 0, "ES hedge must suppress");
        }

        // ----- MUST NOT flag: suppressors -----

        #[test]
        fn opinion_frames_suppress_en_and_fr() {
            let opinions = [
                "I recommend the route should be handled by a dedicated middleware.",
                "In my opinion the parameter should be configured at the infra layer.",
                "Je recommande que la table soit gérée par un service dédié.",
                "À mon avis le paramètre devrait être configuré ailleurs.",
            ];
            for o in opinions {
                assert_eq!(lint_assertions(o).unsourced_count, 0, "opinion must not flag: {o}");
            }
        }

        #[test]
        fn conditionals_suppress_fr_and_en() {
            let conds = [
                "Si le paramètre est configuré à zéro, la requête ne coupe jamais vraiment.",
                "Quand le cache est géré par le DOM, le comportement change pour tout le monde.",
                "If the parameter is configured to zero, the request never times out at all.",
                "When the route is handled upstream, the gateway returns early for callers.",
            ];
            for c in conds {
                assert_eq!(lint_assertions(c).unsourced_count, 0, "conditional must not flag: {c}");
            }
        }

        #[test]
        fn questions_suppress() {
            let qs = [
                "Where exactly is the auth token stored in the session table anyway?",
                "Est-ce que la route est vraiment gérée par le middleware dédié ?",
            ];
            for q in qs {
                assert_eq!(lint_assertions(q).unsourced_count, 0, "question must not flag: {q}");
            }
        }

        #[test]
        fn markdown_headings_suppress() {
            let headings = [
                "### The endpoint returns a paginated list of widgets for the caller",
                "## Où la route est définie côté backend pour tous les appels",
                "The endpoint returns a paginated list of widgets for the caller**",
            ];
            for h in headings {
                assert_eq!(lint_assertions(h).unsourced_count, 0, "heading must not flag: {h}");
            }
        }

        #[test]
        fn imperative_bullets_suppress() {
            let bullets = [
                "- Configure the endpoint that returns the paginated widget list for callers",
                "* Vérifier que la route est gérée par le middleware dédié partout",
                "Ajouter le paramètre qui est configuré pour tout le cluster ici",
            ];
            for b in bullets {
                assert_eq!(lint_assertions(b).unsourced_count, 0, "imperative must not flag: {b}");
            }
        }

        #[test]
        fn hedges_suppress_all_accent_variants() {
            // peut-être / peut être / peut etre must all suppress equally.
            let hedged = [
                "Le cache est peut-être géré localement, ça reste à confirmer côté serveur.",
                "Le cache est peut être géré localement, ça reste à confirmer côté serveur.",
                "Le cache est peut etre géré localement, ça reste à confirmer côté serveur.",
                "I think the endpoint returns a cached value, but I should check first.",
            ];
            for h in hedged {
                assert_eq!(lint_assertions(h).unsourced_count, 0, "hedge must not flag: {h}");
            }
        }

        #[test]
        fn anchors_suppress_every_form() {
            let root = temp_project();
            // backtick path, [src:] marker, URL — each suppresses the flag.
            let anchored = [
                "The endpoint returns the widget list in `backend/src/api/widgets.rs`.",
                "The endpoint returns the widget list [src: file: src/foo.rs:1].",
                "The endpoint returns the widget list per https://api.example.com/docs page.",
            ];
            for a in anchored {
                let r = analyze(a, Some(&root));
                assert_eq!(r.unsourced_count, 0, "anchor must suppress: {a} -> {:?}", r.flagged_spans);
            }
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn exact_di_sentence_regression() {
            // The canonical DI false-positive: opinion inside a conditional.
            let t = "- La conformité DI n'est pas toujours une amélioration — si le \
                     cycle de vie est géré par le DOM, DI peut être un anti-pattern ici";
            assert_eq!(lint_assertions(t).unsourced_count, 0, "{:?}", lint_assertions(t));
        }

        #[test]
        fn version_si_boundary_regression() {
            // "si" must NOT match inside "version" and wrongly suppress a real claim.
            let t = "La version 7.3.6 est vulnérable à une faille de désérialisation critique.";
            assert!(lint_assertions(t).unsourced_count >= 1, "{:?}", lint_assertions(t));
        }

        #[test]
        fn conditional_after_cue_still_flags() {
            // The conditional guard only suppresses when the opener sits BEFORE
            // the cue. A claim cue first, with a trailing conditional clause,
            // still flags (it's an assertion with a caveat, not a hypothesis).
            let t = "The auth token is stored in the session table, if you must know the detail.";
            assert_eq!(lint_assertions(t).unsourced_count, 1, "{:?}", lint_assertions(t));
        }
    }

    // ── 7. analyze / finalize / report-aggregation matrix ────────────────
    mod report_aggregation_matrix {
        use super::*;

        #[test]
        fn mixed_report_counts_all_three_signals() {
            let root = temp_project();
            let txt = "Verified [src: file: src/foo.rs:2]. \
                       Fabricated [src: file: src/ghost.rs:1]. \
                       The worker pool is implemented with great care for the whole fleet.";
            let r = analyze(txt, Some(&root));
            assert_eq!(r.fabricated_count, 1, "sources: {:?}", r.sources);
            assert_eq!(r.verified_count(), 1, "sources: {:?}", r.sources);
            assert_eq!(r.unsourced_count, 1, "spans: {:?}", r.flagged_spans);
            assert!(r.has_signal());
            assert!(!r.is_empty());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn only_unchecked_has_signal_option_b() {
            let r = analyze("Docs [src: url: https://x.io] and [src: commit: abc123].", None);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unsourced_count, 0);
            assert_eq!(r.verified_count(), 0);
            // Option B: surfaced (neutral "unverifiable"), not silent.
            assert!(r.has_signal());
            assert!(!r.is_empty());
        }

        #[test]
        fn verified_only_is_green_signal() {
            let root = temp_project();
            let r = analyze("Retry [src: file: src/foo.rs:2] and `src/foo.rs:3` again.", Some(&root));
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unsourced_count, 0);
            assert_eq!(r.verified_count(), 1, "deduped to one: {:?}", r.sources);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn max_sources_verified_cap_is_respected() {
            // Emit far more than MAX_SOURCES_VERIFIED markers; the verified
            // sources vec must never exceed the cap.
            let root = temp_project();
            let mut txt = String::new();
            for _ in 0..(MAX_SOURCES_VERIFIED + 20) {
                txt.push_str("[src: file: src/foo.rs:1] ");
            }
            let r = analyze(&txt, Some(&root));
            assert!(
                r.sources.len() <= MAX_SOURCES_VERIFIED,
                "sources len {} must be capped at {}",
                r.sources.len(),
                MAX_SOURCES_VERIFIED,
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn fabricated_and_verified_both_present_in_signal() {
            let root = temp_project();
            let txt = "Good [src: file: src/foo.rs:1]. Bad [src: file: src/nope.rs:1].";
            let r = analyze(txt, Some(&root));
            assert_eq!(r.verified_count(), 1);
            assert_eq!(r.fabricated_count, 1);
            assert!(r.has_signal(), "both signals present");
            std::fs::remove_dir_all(&root).ok();
        }

        // ONE sequential test for the mode-global drift caveat.
        #[serial_test::serial] // mutates the process-global anti-halluc mode
        #[test]
        fn finalize_mode_off_then_warn_sequential() {
            let root = temp_project();
            // off → None even for a perfect citation.
            set_mode("off");
            assert!(
                finalize_lint_report(
                    "Retry [src: file: src/foo.rs:2].",
                    None,
                    root.to_str().unwrap(),
                    &[],
                )
                .is_none(),
                "off mode must produce no report",
            );
            // warn → green stored report.
            set_mode("warn");
            let rep = finalize_lint_report(
                "Retry [src: file: src/foo.rs:2].",
                None,
                root.to_str().unwrap(),
                &[],
            )
            .expect("warn mode must store a verified report");
            assert!(rep.verified_count() >= 1, "{:?}", rep.sources);
            // no-signal prose → None even in warn.
            assert!(
                finalize_lint_report("Voilà, terminé pour aujourd'hui.", None, "", &[]).is_none(),
                "no-signal prose must produce no report",
            );
            std::fs::remove_dir_all(&root).ok();
            // Restore rollout default for any sibling test reading the global mode.
            set_mode("warn");
        }
    }

    // ── per-return-type scenario matrix ───────────────────────────────
    //
    // QA hardening (0.8.7): for EACH of the six anti-hallu pill states the
    // UI can render, 2-3 DETERMINISTIC tests assert the exact counts +
    // statuses an `analyze`/`analyze_roots` call produces against a seeded
    // temp project. The six states:
    //   1. VERIFIED   (green)      — a source mechanically resolved.
    //   2. UNSOURCED  (amber)      — a claim cue with no anchor.
    //   3. FABRICATED (red)        — a FORMAL [src:] failed verification.
    //   4. UNVERIFIED (soft amber) — a NON-resolving INLINE backtick anchor.
    //   5. NO SIGNAL  (no pill)    — plain prose, nothing to show.
    //   6. UNCHECKED  (no pill)    — only soft/non-file tiers (url/user/…).
    // Plus mixed-report scenarios proving the four counts coexist correctly.
    //
    // These tests are mode-INDEPENDENT (they call `analyze`/`analyze_roots`
    // directly, never `finalize_lint_report` except where the gate itself is
    // under test, and the one such test sets the mode locally + restores it).
    mod scenario_matrix {
        use super::*;

        /// A richer seeded project than `temp_project()`: adds a second source
        /// file so "multiple verified sources" is expressible.
        fn temp_project_multi() -> PathBuf {
            let root = temp_project(); // src/foo.rs = 5 lines
            // A 3-line file at src/bar.ts.
            std::fs::write(root.join("src/bar.ts"), "x\ny\nz\n").unwrap();
            root
        }

        // ── 1. VERIFIED (green) ───────────────────────────────────────

        #[test]
        fn verified_formal_marker_resolves_green() {
            let root = temp_project();
            let r = analyze("The retry path is [src: file: src/foo.rs:2].", Some(&root));
            assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unsourced_count, 0);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn verified_inline_backtick_anchor_resolves_green() {
            // Niveau 1.5: a NATURAL backticked `path:line` that resolves is a
            // green verified source (auto-verified inline anchor), no formal
            // [src:] needed.
            let root = temp_project();
            let r = analyze("See `src/foo.rs:3` for details.", Some(&root));
            assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal());
            assert!(
                r.sources[0].detail.contains("inline anchor (auto-verified)"),
                "{:?}",
                r.sources[0],
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn verified_multiple_distinct_sources_all_green() {
            // Two DISTINCT files (dedup is by bare path, so they don't collapse).
            let root = temp_project_multi();
            let r = analyze(
                "Logic in [src: file: src/foo.rs:1] and helper `src/bar.ts:2`.",
                Some(&root),
            );
            assert_eq!(r.verified_count(), 2, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        // ── 2. UNSOURCED (amber) ──────────────────────────────────────

        #[test]
        fn unsourced_english_claim_no_anchor_amber() {
            let root = temp_project();
            // "the function" cue, no anchor, long enough, not a question/heading.
            let r = analyze(
                "The function returns the cached connection pool handle every time.",
                Some(&root),
            );
            assert!(r.unsourced_count >= 1, "{:?}", r.flagged_spans);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.verified_count(), 0);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unsourced_french_claim_no_anchor_amber() {
            let root = temp_project();
            // "se trouve" cue, no anchor.
            let r = analyze(
                "La configuration du cache se trouve directement dans le module principal.",
                Some(&root),
            );
            assert!(r.unsourced_count >= 1, "{:?}", r.flagged_spans);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.verified_count(), 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unsourced_count_matches_number_of_claims() {
            let root = temp_project();
            // Two independent unsourced claims (separate sentences, separate cues).
            let r = analyze(
                "The endpoint is defined in the gateway layer. \
                 La méthode renvoie systématiquement une valeur encodée.",
                Some(&root),
            );
            assert_eq!(r.unsourced_count, 2, "{:?}", r.flagged_spans);
            assert_eq!(r.fabricated_count, 0);
            std::fs::remove_dir_all(&root).ok();
        }

        // ── 3. FABRICATED (red) — FORMAL [src:] only ──────────────────

        #[test]
        fn fabricated_not_found_red() {
            let root = temp_project();
            let r = analyze("Defined in [src: file: ghost.rs:1].", Some(&root));
            assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
            assert_eq!(r.sources[0].status, SourceStatus::NotFound);
            assert!(r.sources[0].status.is_fabricated());
            assert_eq!(r.verified_count(), 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn fabricated_out_of_bounds_and_outside_project_red() {
            let root = temp_project();
            // OutOfBounds (foo.rs is 5 lines) + OutsideProject (../ escape).
            let r = analyze(
                "Lines [src: file: src/foo.rs:9999] and [src: file: ../../etc/passwd:1].",
                Some(&root),
            );
            assert_eq!(r.fabricated_count, 2, "{:?}", r.sources);
            let statuses: Vec<_> = r.sources.iter().map(|s| s.status).collect();
            assert!(statuses.contains(&SourceStatus::OutOfBounds), "{statuses:?}");
            assert!(statuses.contains(&SourceStatus::OutsideProject), "{statuses:?}");
            assert!(r.sources.iter().all(|s| s.status.is_fabricated()));
            assert_eq!(r.verified_count(), 0);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn fabricated_training_data_is_rejected_red() {
            let root = temp_project();
            let r = analyze("This is well known [src: training-data: GPT prior].", Some(&root));
            assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
            assert_eq!(r.sources[0].kind, SourceKind::TrainingData);
            assert_eq!(r.sources[0].status, SourceStatus::Rejected);
            assert!(r.sources[0].status.is_fabricated());
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        // ── 4. UNVERIFIED (soft amber, NEW) — INLINE anchor only ──────

        #[test]
        fn unverified_inline_not_found_soft_amber_not_red() {
            let root = temp_project();
            // NON-resolving INLINE backtick anchor → unverified, NOT fabricated.
            let r = analyze("Check `src/ghost.rs:1` for the handler.", Some(&root));
            assert_eq!(r.unverified_count, 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0, "inline must NOT escalate to red");
            assert_eq!(r.verified_count(), 0);
            assert!(r.has_signal());
            // The source IS listed, status Unchecked, detail says "couldn't verify".
            assert_eq!(r.sources.len(), 1);
            assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
            assert!(
                r.sources[0].detail.contains("couldn't verify"),
                "{:?}",
                r.sources[0],
            );
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unverified_inline_out_of_bounds_soft_amber_not_red() {
            let root = temp_project();
            // Existing file, line beyond length, but cited INLINE → soft amber.
            let r = analyze("The fix is at `src/foo.rs:9999` in the loop.", Some(&root));
            assert_eq!(r.unverified_count, 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
            assert!(r.sources[0].detail.contains("couldn't verify"));
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unverified_two_inline_anchors_both_soft() {
            let root = temp_project();
            let r = analyze(
                "See `src/ghost.rs:1` and also `src/missing.ts:2`.",
                Some(&root),
            );
            assert_eq!(r.unverified_count, 2, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.verified_count(), 0);
            assert!(r.sources.iter().all(|s| s.status == SourceStatus::Unchecked));
            std::fs::remove_dir_all(&root).ok();
        }

        // ── 5. NO SIGNAL (no pill) ────────────────────────────────────

        #[test]
        fn no_signal_plain_prose_is_empty() {
            let root = temp_project();
            let r = analyze("Voilà, c'est terminé pour aujourd'hui. Bonne soirée à tous.", Some(&root));
            assert!(!r.has_signal());
            assert!(r.is_empty());
            assert_eq!(r.unsourced_count, 0);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unverified_count, 0);
            assert_eq!(r.verified_count(), 0);
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn no_signal_english_smalltalk_is_empty() {
            let root = temp_project();
            let r = analyze("Thanks, that all looks good to me. Have a great weekend!", Some(&root));
            assert!(!r.has_signal());
            assert!(r.is_empty());
            std::fs::remove_dir_all(&root).ok();
        }

        #[serial_test::serial] // mutates the process-global anti-halluc mode
        #[test]
        fn no_signal_finalize_returns_none() {
            // The has_signal gate in finalize_lint_report drops a no-signal
            // report. Mode-dependent → set + restore locally, kept in ONE fn.
            let root = temp_project();
            set_mode("warn");
            let out = finalize_lint_report(
                "Voilà, c'est terminé pour aujourd'hui. Bonne soirée.",
                None,
                root.to_str().unwrap(),
                &[],
            );
            assert!(out.is_none(), "no-signal prose must finalize to None");
            set_mode(DEFAULT_MODE_STR);
            std::fs::remove_dir_all(&root).ok();
        }

        // ── 6. UNCHECKED / non-vérifiable (no pill, listed for drawer) ─

        #[test]
        fn unchecked_url_and_user_no_signal_but_listed() {
            let root = temp_project();
            let r = analyze(
                "Docs [src: url: https://example.test/x] and [src: user-confirmed 2026-01-01].",
                Some(&root),
            );
            // Neither fabricated nor verified nor unverified → no pill.
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.verified_count(), 0);
            assert_eq!(r.unverified_count, 0);
            assert_eq!(r.unsourced_count, 0);
            assert!(r.has_signal(), "Option B: unchecked-only is surfaced (neutral pill)");
            // The sources ARE listed for drawer transparency.
            assert_eq!(r.sources.len(), 2, "{:?}", r.sources);
            assert!(r.sources.iter().all(|s| s.status == SourceStatus::Unchecked));
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unchecked_inferred_and_commit_no_signal_but_listed() {
            let root = temp_project();
            let r = analyze(
                "Probably [src: inferred: derived from the trait bound] per [src: commit: abc1234].",
                Some(&root),
            );
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.verified_count(), 0);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal()); // Option B: surfaced
            assert_eq!(r.sources.len(), 2, "{:?}", r.sources);
            let kinds: Vec<_> = r.sources.iter().map(|s| s.kind).collect();
            assert!(kinds.contains(&SourceKind::Inferred), "{kinds:?}");
            assert!(kinds.contains(&SourceKind::Commit), "{kinds:?}");
            assert!(r.sources.iter().all(|s| s.status == SourceStatus::Unchecked));
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn unchecked_hypothesis_only_no_signal() {
            let root = temp_project();
            let r = analyze("It may be [src: hypothesis: the cache is invalidated on write].", Some(&root));
            assert_eq!(r.sources.len(), 1, "{:?}", r.sources);
            assert_eq!(r.sources[0].kind, SourceKind::Hypothesis);
            assert_eq!(r.sources[0].status, SourceStatus::Unchecked);
            assert!(r.has_signal()); // Option B: surfaced (neutral)
            std::fs::remove_dir_all(&root).ok();
        }

        // ── MIXED reports — all four counts coexist ───────────────────

        #[test]
        fn mixed_verified_unsourced_fabricated_unverified_all_counts() {
            let root = temp_project_multi();
            // - verified : formal [src: file: src/foo.rs:2]   → green
            // - fabricated: formal [src: file: ghost.rs:1]    → red (NotFound)
            // - unverified: inline `src/missing.ts:3`         → soft amber
            // - unsourced : a bare claim sentence with a cue, no anchor → amber
            let text = "\
The pool lives in [src: file: src/foo.rs:2]. \
Handler at [src: file: ghost.rs:1]. \
See `src/missing.ts:3` too.
La méthode renvoie toujours une connexion réutilisée.";
            let r = analyze(text, Some(&root));
            assert_eq!(r.verified_count(), 1, "verified: {:?}", r.sources);
            assert_eq!(r.fabricated_count, 1, "fabricated: {:?}", r.sources);
            assert_eq!(r.unverified_count, 1, "unverified: {:?}", r.sources);
            assert_eq!(r.unsourced_count, 1, "unsourced: {:?}", r.flagged_spans);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn mixed_red_dominates_but_green_still_counted() {
            // Priority sanity: a report with BOTH a fabricated and a verified
            // source keeps both counts truthful (the UI decides pill priority;
            // the report stays honest — red present, green present).
            let root = temp_project();
            let text = "Good [src: file: src/foo.rs:1] but bad [src: file: ghost.rs:9].";
            let r = analyze(text, Some(&root));
            assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 1, "{:?}", r.sources);
            assert_eq!(r.unverified_count, 0);
            assert!(r.has_signal());
            std::fs::remove_dir_all(&root).ok();
        }

        #[test]
        fn mixed_unchecked_tier_does_not_inflate_any_count() {
            // A report mixing one VERIFIED file ref with soft/unchecked tiers
            // (url, inferred) → only the file counts as verified; the unchecked
            // tiers are listed but do NOT bump verified/fabricated/unverified.
            let root = temp_project();
            let text = "\
Logic in [src: file: src/foo.rs:1], see [src: url: https://example.test/y], \
probably [src: inferred: from the signature].";
            let r = analyze(text, Some(&root));
            assert_eq!(r.verified_count(), 1, "{:?}", r.sources);
            assert_eq!(r.fabricated_count, 0);
            assert_eq!(r.unverified_count, 0);
            assert_eq!(r.sources.len(), 3, "all three listed: {:?}", r.sources);
            assert!(r.has_signal(), "the one verified file gives a green signal");
            std::fs::remove_dir_all(&root).ok();
        }
    }
}
