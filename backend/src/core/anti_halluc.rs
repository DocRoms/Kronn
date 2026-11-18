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
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "off" | "warn" | "enforce"
    )
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

/// The only way a test changes the mode: it holds a process-wide lock, so two
/// tests never interleave their modes, and puts the default back on drop.
#[cfg(any(test, feature = "test-support"))]
pub struct ModeOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(any(test, feature = "test-support"))]
static MODE_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static MODE_OVERRIDE_HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(any(test, feature = "test-support"))]
impl ModeOverride {
    pub fn set(mode: &str) -> Self {
        // A second override on the same thread would wait on itself forever.
        assert!(
            !MODE_OVERRIDE_HELD.get(),
            "one ModeOverride per test: change it with switch()"
        );
        let lock = MODE_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        MODE_OVERRIDE_HELD.set(true);
        set_mode(mode);
        Self { _lock: lock }
    }

    pub fn switch(&self, mode: &str) {
        set_mode(mode);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for ModeOverride {
    fn drop(&mut self) {
        set_mode(DEFAULT_MODE_STR);
        MODE_OVERRIDE_HELD.set(false);
    }
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
    /// `deps: <manager>/<package>@<YYYY-MM-DD>` — a row of Kronn's dependency
    /// watch; only an audit holding that snapshot can verify it.
    Deps,
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
    /// Tree the cited file was found in. Absent on non-file sources, on
    /// unresolved files and on rows stored before it existed: read as checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub origin: Option<SourceOrigin>,
}

/// Which tree held a cited file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SourceOriginKind {
    /// The project's main checkout (or the discussion's own working tree).
    Checkout,
    /// An unmerged worktree of the same project.
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct SourceOrigin {
    pub kind: SourceOriginKind,
    /// Worktree directory name; absent for the checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
}

impl SourceOrigin {
    pub fn checkout() -> Self {
        Self {
            kind: SourceOriginKind::Checkout,
            name: None,
        }
    }

    pub fn worktree(name: &str) -> Self {
        Self {
            kind: SourceOriginKind::Worktree,
            name: Some(name.to_string()),
        }
    }

    fn is_checkout(&self) -> bool {
        self.kind == SourceOriginKind::Checkout
    }
}

/// A worktree of the discussion's own project, consulted after the checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneRoot {
    pub name: String,
    pub path: PathBuf,
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
    /// KT-1401 — `kronn-question` fences of the message that Kronn could not
    /// turn into an arbitration card, each with the reason. Set when the
    /// message is stored, so the author is told instead of leaving a card the
    /// human sees but cannot answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<InvalidCard>>", optional)]
    pub invalid_cards: Vec<InvalidCard>,
}

/// A `kronn-question` fence that no card could be recorded from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct InvalidCard {
    /// The fence's `key`, when it could still be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key: Option<String>,
    pub reason: String,
}

impl LintReport {
    pub fn empty() -> Self {
        Self {
            unsourced_count: 0,
            flagged_spans: Vec::new(),
            sources: Vec::new(),
            fabricated_count: 0,
            unverified_count: 0,
            invalid_cards: Vec::new(),
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
        self.unsourced_count > 0 || !self.sources.is_empty() || !self.invalid_cards.is_empty()
    }

    /// Back-compat: "no signal at all". Note the flipped semantics vs the
    /// pre-0.8.8 version — an all-verified report is NO LONGER empty.
    pub fn is_empty(&self) -> bool {
        !self.has_signal()
    }
}

pub const SPAN_EXCERPT_MAX_CHARS: usize = 160;
/// Cap on reported spans — beyond this the count still grows but we stop
/// storing excerpts (keeps the JSON small on a pathological message).
pub const MAX_FLAGGED_SPANS: usize = 25;
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
    "is located",
    "is defined",
    "is implemented",
    "lives in",
    "is stored",
    "is configured",
    "defaults to",
    "the default is",
    "returns ",
    "is handled",
    "the function",
    "the method",
    "the endpoint",
    "the route",
    "the table",
    "the column",
    "the config",
    "the parameter",
    "the flag",
    "the option",
    "the api",
    "is set in",
    "is declared",
    // ── English — security / CVE / version (0.8.7 patch) ──
    //
    // Kept high-precision on purpose : broader cues like "added in" /
    // "available in" / "the current version" / "ships with" fire on
    // generic prose ("we added in a new flag", "available in the docs")
    // and would spam the pill — explicitly NOT in this list. The cues
    // below are tight verb-frames ("is vulnerable", "fixed in",
    // "patched in") or explicit version-state framings ("the latest
    // version is …") that don't collide with everyday prose.
    "is vulnerable",
    "is affected by",
    "patched in",
    "fixed in",
    "deprecated in version",
    "affected version",
    "affected versions",
    "the latest version is",
    "the installed version is",
    "cve-",
    // ── French — code / config (original list) ──
    "se trouve",
    "est défini",
    "est implémenté",
    "est stocké",
    "est configuré",
    "par défaut",
    "renvoie",
    "la fonction",
    "la méthode",
    "la route",
    "la table",
    "la colonne",
    "le paramètre",
    "l'option",
    "l'endpoint",
    "est déclaré",
    "est géré",
    // ── French — security / CVE / version (0.8.7 patch) ──
    //
    // Same precision discipline as the English block above. "ajouté dans"
    // / "disponible dans" / "la version actuelle" are intentionally NOT
    // here — they collide with generic prose. The cues below are tight
    // verb-frames ("est vulnérable", "corrigé dans", "patché dans") or
    // explicit version-state framings ("la dernière version est …").
    "est vulnérable",
    "est affecté par",
    "patché dans",
    "corrigé dans",
    "déprécié dans la version",
    "version affectée",
    "versions affectées",
    "la dernière version est",
    "la version installée est",
    // ── Spanish — code / config (0.8.8 — closes the ES recall gap) ──
    //
    // Kronn is FR/EN/ES; before this block a genuine Spanish code claim
    // ("la función está definida en …") flagged via NOTHING (only language-
    // agnostic tokens like `cve-`/backticked-path fired). Same precision
    // discipline as the FR/EN blocks: tight verb-frames + state framings, no
    // generic prose. The matcher is accent-insensitive (normalize_match) so
    // "está" matches whether or not the accent survives.
    "se encuentra",
    "está definido",
    "está definida",
    "está implementado",
    "está almacenado",
    "está configurado",
    "está configurada",
    "por defecto",
    "devuelve",
    "la función",
    "el método",
    "la ruta",
    "la tabla",
    "la columna",
    "el parámetro",
    "la opción",
    "está declarado",
    "está gestionado",
    "es vulnerable",
    "está afectado por",
    "corregido en",
    "parcheado en",
    "versión afectada",
    "versiones afectadas",
    "la última versión es",
];

/// Hedges (EN + FR + ES). Their presence suppresses a flag — the agent is
/// already signalling uncertainty, which is exactly the behaviour we want.
const HEDGES: &[&str] = &[
    "i think",
    "i believe",
    "i'm not sure",
    "im not sure",
    "not sure",
    "maybe",
    "probably",
    "might be",
    "i should check",
    "unverified",
    "to verify",
    "let me check",
    "i'll check",
    "appears to",
    "seems to",
    "je pense",
    "je crois",
    "peut-être",
    "peut etre",
    "probablement",
    "il semble",
    "à vérifier",
    "a verifier",
    "je vérifie",
    "je verifie",
    "sans certitude",
    "je ne suis pas sûr",
    "je ne suis pas sur",
    // ── Spanish ──
    "creo que",
    "quizás",
    "quizas",
    "tal vez",
    "probablemente",
    "no estoy seguro",
    "parece que",
    "debería revisar",
    "por verificar",
    "sin certeza",
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
pub const SOURCE_EXTS: &[&str] = &[
    ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".toml", ".json", ".sql", ".md", ".yml", ".yaml",
    ".css", ".html", ".sh", ".php", ".go", ".java", ".rb", ".vue", ".svelte", ".c", ".cpp", ".h",
    // Web / template / i18n project files (Symfony, Laravel, Sass, XLIFF…).
    ".twig", ".xlf", ".scss", ".less",
];

/// Heuristic: a file path, a `path/like.ext` token, a namespaced identifier,
/// or a `fn()` call ref — any concrete, checkable reference.
pub fn contains_code_anchor(s: &str) -> bool {
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
    "peut etre",
    "pourrait etre",
    "n'est pas toujours",
    "pas forcement",
    "pas necessairement",
    "anti pattern",
    "il vaut mieux",
    "il faudrait",
    "on devrait",
    "tu devrais",
    "vous devriez",
    "devrait",
    "je recommande",
    "je recommanderais",
    "je suggere",
    "a mon avis",
    "selon moi",
    "ce serait mieux",
    "ferait mieux",
    "il serait preferable",
    // ── English ──
    "i recommend",
    "i suggest",
    "should be",
    "would be better",
    "in my opinion",
    "not always",
    "we should",
    "you should",
    "it would be",
    // ── Spanish (0.8.8 — precision guard for the new ES claim cues) ──
    "deberia",
    "seria mejor",
    "es preferible",
    "recomiendo",
    "sugiero",
    "en mi opinion",
    "a mi juicio",
    "no siempre",
    "mejor seria",
];

/// Conditional / hypothetical openers (bare tokens — matched at word boundaries
/// via [`find_first_word`], NOT as substrings, so "si" doesn't fire inside
/// "ver**si**on"). A claim cue that sits AFTER one of these in the same sentence
/// is hypothetical ("si X est géré par le DOM, …"), not an assertion.
const CONDITIONAL_OPENERS: &[&str] = &[
    "si",
    "s'il",
    "lorsque",
    "lorsqu'",
    "quand",
    "des que",
    "tant que",
    "a condition",
    "au cas ou",
    "supposons",
    "imaginons",
    "if",
    "when",
    "whenever",
    "assuming",
    "suppose",
    // ── Spanish ── ("si" already shared with FR)
    "cuando",
    "mientras",
    "en caso de",
    "supongamos",
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
        "specifier",
        "verifier",
        "ajouter",
        "creer",
        "definir",
        "configurer",
        "mettre",
        "faire",
        "voir",
        "utiliser",
        "remplacer",
        "supprimer",
        "add",
        "verify",
        "create",
        "define",
        "configure",
        "use",
        "replace",
        "remove",
        "check",
        "ensure",
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
pub fn strip_fenced_code(text: &str) -> String {
    let scan = scan_fences(text, &['`'], false);
    let mut out = String::with_capacity(text.len());
    for (line, fenced) in text.lines().zip(&scan.in_fence) {
        if !fenced {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Where the fenced blocks of a document sit, line by line.
pub struct FenceScan {
    /// One entry per `text.lines()` line: `true` when the line belongs to a
    /// fenced block (its opening and closing fence lines included).
    pub in_fence: Vec<bool>,
    /// A fence opened and the document ended before a valid closer.
    pub unclosed: bool,
}

/// The fence state machine behind [`strip_fenced_code`], exposed per line so
/// a caller that needs line INDICES (the `kronn:section` ownership parser)
/// can ignore what sits inside code without losing its position in the file.
///
/// `fence_chars` picks the fence characters recognised (`` ` `` only for the
/// prose linters; `` ` `` + `~` for ownership, where a `~~~` block hides a
/// section from a renderer just as well). An UNCLOSED fence is reported in
/// `unclosed`; its lines count as code only when `unclosed_is_code` (strict,
/// CommonMark: the block runs to the end of the document) — otherwise they are
/// restored as visible, the fail-closed choice for a consumer that must not
/// lose sight of real content behind a stray fence.
pub fn scan_fences(text: &str, fence_chars: &[char], unclosed_is_code: bool) -> FenceScan {
    /// `Some((char, run_len, suffix))` when the line is a fence marker
    /// candidate: ≤3 leading spaces then a run of ≥3 fence characters. Tabs
    /// disqualify.
    fn fence_marker<'a>(line: &'a str, fence_chars: &[char]) -> Option<(char, usize, &'a str)> {
        let stripped = line.trim_start_matches(' ');
        if line.len() - stripped.len() > 3 {
            return None; // 4+ spaces = indented code, not a fence
        }
        let fence_char = stripped
            .chars()
            .next()
            .filter(|c| fence_chars.contains(c))?;
        let run = stripped.chars().take_while(|&c| c == fence_char).count();
        if run < 3 {
            return None;
        }
        // Fence characters are ASCII: `run` chars == `run` bytes.
        Some((fence_char, run, &stripped[run..]))
    }

    let mut in_fence: Vec<bool> = Vec::new();
    // (opening char, opening run length, index of the opening line)
    let mut fence: Option<(char, usize, usize)> = None;
    for (idx, line) in text.lines().enumerate() {
        match fence {
            None => {
                // A backtick fence's info string may not contain a backtick
                // (CommonMark): ```lang`oops is CONTENT, not an opener — a
                // false opener must never swallow a real slot.
                match fence_marker(line, fence_chars) {
                    Some((fence_char, run, info)) if !(fence_char == '`' && info.contains('`')) => {
                        fence = Some((fence_char, run, idx));
                        in_fence.push(true);
                    }
                    _ => in_fence.push(false),
                }
            }
            Some((open_char, open_run, _)) => {
                in_fence.push(true);
                if let Some((fence_char, run, suffix)) = fence_marker(line, fence_chars) {
                    // Closer suffix: spaces/tabs ONLY — Unicode whitespace
                    // must not promote a content line into a closer.
                    if fence_char == open_char
                        && run >= open_run
                        && suffix.chars().all(|c| c == ' ' || c == '\t')
                    {
                        fence = None; // valid closer: the block is code
                    }
                }
            }
        }
    }
    let unclosed = fence.is_some();
    if let Some((_, _, start)) = fence {
        if !unclosed_is_code {
            // Withheld lines (opening line included) are restored verbatim.
            in_fence[start..].fill(false);
        }
    }
    FenceScan { in_fence, unclosed }
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
pub fn strip_inline_code(text: &str) -> String {
    partition_inline_code(text).0
}

/// Count `{{IDENT}}` shaped placeholders in a markdown body. Conservative:
/// only counts uppercase-snake tokens to avoid matching legitimate Twig /
/// Liquid / Mermaid syntax that uses `{{` for its own purposes (e.g.
/// `{{ asset('foo') }}` in a Twig snippet inside coding-rules.md, or
/// `{{ DECISION_1 }}` is matched, but `{{ asset(...) }}` is not).
/// Fenced blocks and inline code spans are invisible to the counter —
/// documenting a placeholder is not the same as leaving one unfilled
/// (benchmark run 1 false positive: the TD index mentioning
/// `{{DO_NOT_1}}` in inline code made a 43-TD file read as "still the
/// template"). Reuses the anti-hallu strippers: inline spans pair
/// N-backtick runs (an UNCLOSED run keeps its text — a stray backtick can
/// never hide a real slot, bounded pairing on hostile lines); fences are
/// lexical, and an UNCLOSED fence restores everything it withheld,
/// opening line included.
pub fn count_raw_placeholders(content: &str) -> usize {
    raw_placeholder_names(content).len()
}

/// The `{{TOKEN}}` placeholders still in `content`, in order, outside code.
pub fn raw_placeholder_names(content: &str) -> Vec<String> {
    let stripped = strip_inline_code(&strip_fenced_code(content));
    // Match `{{IDENT}}` and `{{ IDENT }}` where IDENT is
    // UPPERCASE_SNAKE (with optional digits + _). The trailing
    // boundary is a literal `}}`, not just `}`, to avoid hits on
    // Twig double-brace blocks that contain spaces / parens.
    let mut names = Vec::new();
    let mut rest = stripped.as_str();
    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        // Find closing `}}`
        let Some(end) = after_open.find("}}") else {
            break;
        };
        let inside = after_open[..end].trim();
        // Must be a single UPPERCASE_SNAKE token (letters / digits / _).
        // No spaces, no parens, no quotes. This excludes Twig.
        let is_placeholder = !inside.is_empty()
            && inside
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && inside.chars().any(|c| c.is_ascii_uppercase());
        if is_placeholder {
            names.push(inside.to_string());
        }
        rest = &after_open[end + 2..];
    }
    names
}

pub struct SectionSpan {
    pub name: String,
    pub is_human: bool,
    pub start: usize,
    pub end: usize,
}

/// Fence characters that hide a `kronn:section` marker from the ownership
/// parser. A `~~~` block is code to a renderer exactly like a ``` one.
const OWNERSHIP_FENCE_CHARS: [char; 2] = ['`', '~'];

/// Which lines of `text` sit inside a fenced code block. A marker on such a
/// line is documentation (an example, a quoted report), never a live section
/// boundary: it neither opens nor closes a protected section.
///
/// An unclosed fence is read leniently (its lines stay visible, so a stray
/// fence in a human's own document does not strip protection) — except when
/// `strict_unclosed` is set, where it is CommonMark's block-to-end-of-file.
pub fn code_line_mask(text: &str, strict_unclosed: bool) -> FenceScan {
    scan_fences(text, &OWNERSHIP_FENCE_CHARS, strict_unclosed)
}

/// Scan `lines` for `<!-- kronn:section name="X" ... -->` … `<!-- kronn:section:end -->`
/// pairs. `in_code` flags the lines inside fenced code (index-aligned with
/// `lines`, shorter is fine): their markers are ignored on both ends. An
/// opener with no matching closer is skipped — there is nothing well-formed to
/// protect there.
pub fn parse_named_sections(lines: &[&str], in_code: &[bool]) -> Vec<SectionSpan> {
    let is_code = |idx: usize| in_code.get(idx).copied().unwrap_or(false);
    let mut spans = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        if !is_code(i)
            && trimmed.starts_with("<!-- kronn:section")
            && !trimmed.starts_with("<!-- kronn:section:end")
        {
            if let Some(name) = extract_attr(lines[i], "name") {
                let is_human = is_human_owned_marker(lines[i]);
                if let Some(end) = ((i + 1)..lines.len()).find(|&j| {
                    !is_code(j) && lines[j].trim_start().starts_with("<!-- kronn:section:end")
                }) {
                    spans.push(SectionSpan {
                        name,
                        is_human,
                        start: i,
                        end,
                    });
                    i = end + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    spans
}

pub fn extract_attr(line: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = line.find(&needle)? + needle.len();
    let end = line[start..].find('"')?;
    Some(line[start..start + end].to_string())
}

pub fn is_human_owned_marker(line: &str) -> bool {
    line.contains("owner=\"human\"") || line.contains("curated=\"human\"")
}

/// One live named section: its text (markers included) and 0-based line span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedSection {
    pub name: String,
    pub block: String,
    pub human: bool,
    pub start: usize,
    pub end: usize,
}

/// Every live named section of `content`; fenced examples are not sections.
pub fn named_sections(content: &str) -> Vec<NamedSection> {
    let lines: Vec<&str> = content.split('\n').collect();
    let scan = code_line_mask(content, false);
    parse_named_sections(&lines, &scan.in_fence)
        .into_iter()
        .map(|span| NamedSection {
            block: lines[span.start..=span.end].join("\n"),
            name: span.name,
            human: span.is_human,
            start: span.start,
            end: span.end,
        })
        .collect()
}

/// A syntax example such as `<path>:<line>` or `src/{a,b}.rs`, not a citation.
pub fn is_placeholder(citation: &str) -> bool {
    citation.contains(['<', '>', '{', '}', '*', '…']) || citation.contains("...")
}

/// A doc still holding a raw `{{SLOT}}`, a `<!-- TODO:` line or a bare `PLACEHOLDER` line.
pub fn contains_placeholder(s: &str) -> bool {
    count_raw_placeholders(s) > 0
        || strip_inline_code(&strip_fenced_code(s))
            .lines()
            .any(|line| {
                let trimmed = line.trim();
                trimmed.starts_with("<!-- TODO:") || trimmed == "PLACEHOLDER"
            })
}

/// [`strip_inline_code`]'s prose, plus the inner text of every dropped span,
/// one span per line.
pub fn partition_inline_code(text: &str) -> (String, String) {
    let mut out = String::with_capacity(text.len());
    let mut spans = String::new();
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
                let (start, end) = drop_ranges[di];
                let inner: String = chars[start..end].iter().collect();
                spans.push_str(inner.trim_matches('`'));
                spans.push('\n');
                i = end;
                di += 1;
                continue;
            }
            out.push(chars[i]);
            i += 1;
        }
        out.push('\n');
    }
    (out, spans)
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
        if find_first(&nmatch, HEDGES).is_some() || find_first_word(&nmatch, OPINION_CUES).is_some()
        {
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
        invalid_cards: Vec::new(),
    }
}

// ─── Niveau 1 — structured source extraction + mechanical verification ────

/// Hard caps to bound work at message-finalize time.
pub const MAX_SOURCES_VERIFIED: usize = 50;
/// Skip line-count verification above this file size (existence still checked).
pub const LINE_COUNT_SIZE_CAP_BYTES: u64 = 2 * 1024 * 1024;

/// Extract every `[src: …]` marker from the prose, skipping fenced code blocks
/// AND inline-backtick spans (those are [`extract_quoted_source_markers`]),
/// and dropping EMPTY markers — a bare `[src:]` in prose is a mention of the
/// grammar, not a citation. Returns the inner content (after `src:`),
/// trimmed. Bracket-balanced so `[src: a[0]]` reads `a[0]`.
pub fn extract_source_markers(text: &str) -> Vec<String> {
    markers_in(&strip_inline_code(&strip_fenced_code(text)))
}

/// The `[src: …]` markers quoted in inline-backtick spans. Docs quote real
/// citations this way, but also the grammar itself as an example.
pub fn extract_quoted_source_markers(text: &str) -> Vec<String> {
    markers_in(&partition_inline_code(&strip_fenced_code(text)).1)
}

fn markers_in(prose: &str) -> Vec<String> {
    markers_at(prose)
        .into_iter()
        .map(|(_, inner)| inner)
        .collect()
}

/// [`markers_in`] with the char offset of each marker's `[`.
pub fn markers_at(prose: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = prose.chars().collect();
    let lower: Vec<char> = prose.to_ascii_lowercase().chars().collect();
    let needle: Vec<char> = "[src:".chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + needle.len() <= chars.len() {
        if lower[i..i + needle.len()] == needle[..] {
            // Read until the matching ']' (balance nested '['). A marker never
            // swallows the next `[src:` nor a paragraph break: an unclosed one
            // (a truncated banner, a half-typed citation) ends there, so the
            // markers after it are still read.
            let mut depth = 1i32;
            let mut j = i + needle.len();
            let mut buf = String::new();
            let mut closed = false;
            while j < chars.len() {
                if j + needle.len() <= chars.len() && lower[j..j + needle.len()] == needle[..] {
                    break;
                }
                if chars[j] == '\n' && blank_line_at(&chars, j) {
                    break;
                }
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
                    out.push((i, inner.to_string()));
                }
                i = j + 1;
            } else {
                // Resync right after the opening: whatever stopped the scan
                // (next marker, blank line, end of text) is read normally.
                i += needle.len();
            }
        } else {
            i += 1;
        }
    }
    out
}

/// `value` cut to `max` characters without ever cutting inside a
/// `[src: …]` marker: when the limit lands in one, the cut moves back to just
/// before its `[`. `ellipsis` appends `…` when something was dropped (it counts
/// toward `max`). A marker already unclosed in `value` is dropped from its `[`
/// on, so the cut text never carries a dangling opener.
/// Every Kronn-generated text that can carry a citation must truncate through
/// this, never through a bare `chars().take(n)`.
pub fn truncate_outside_markers(value: &str, max: usize, ellipsis: bool) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let room = if ellipsis { max.saturating_sub(1) } else { max };
    let chars: Vec<char> = value.chars().collect();
    let mut cut = room;
    let lower: Vec<char> = value.to_ascii_lowercase().chars().collect();
    let needle: Vec<char> = "[src:".chars().collect();
    let mut i = 0usize;
    while i + needle.len() <= chars.len() && i < cut {
        if lower[i..i + needle.len()] != needle[..] {
            i += 1;
            continue;
        }
        let mut depth = 1i32;
        let mut j = i + needle.len();
        let mut close = None;
        while j < chars.len() {
            match chars[j] {
                '[' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(j);
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        match close {
            // The limit falls inside this closed marker: stop before it.
            Some(end) if end >= cut => {
                cut = i;
                break;
            }
            Some(end) => i = end + 1,
            // An unclosed marker is malformed already: never keep its opener.
            None => {
                cut = i;
                break;
            }
        }
    }
    let mut out: String = chars[..cut].iter().collect();
    if ellipsis {
        out = out.trim_end().to_string();
        out.push('…');
    }
    out
}

/// Whether the newline at `at` is followed by a blank line (`\n` + optional
/// spaces + `\n`).
fn blank_line_at(chars: &[char], at: usize) -> bool {
    chars[at + 1..]
        .iter()
        .find(|c| **c == '\n' || !c.is_whitespace())
        == Some(&'\n')
}

/// Classify a marker's inner content into a kind + the bare reference, stripping
/// an optional `<type>:` prefix from the v1.1 grammar (`file:`, `url:`, …).
pub fn classify_source(raw: &str) -> (SourceKind, String) {
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
    if let Some(r) = match_type_keyword(s, "training-data") {
        return (SourceKind::TrainingData, r);
    }
    if let Some(r) = match_type_keyword(s, "code-comment") {
        return (SourceKind::CodeComment, r);
    }
    if let Some(r) = match_type_keyword(s, "inferred") {
        return (SourceKind::Inferred, r);
    }
    if let Some(r) = match_type_keyword(s, "hypothesis") {
        return (SourceKind::Hypothesis, r);
    }
    if let Some(r) = match_type_keyword(s, "commit") {
        return (SourceKind::Commit, r);
    }
    if let Some(r) = match_type_keyword(s, "api") {
        return (SourceKind::Api, r);
    }
    if let Some(r) = match_type_keyword(s, "deps") {
        return (SourceKind::Deps, r);
    }
    if let Some(r) = match_type_keyword(s, "url") {
        return (SourceKind::Url, r);
    }
    if let Some(r) = match_type_keyword(s, "user") {
        return (SourceKind::User, r);
    }
    if let Some(r) = match_type_keyword(s, "file") {
        return (SourceKind::File, r);
    }
    // `<word>: <text>`: only a CLOSED list of non-file kinds (`command: git log`)
    // is never a path to resolve. Any other word is a typo or an invented kind
    // (`fil:`, `path:`, `source:`): if its text can be a path it is verified as
    // a file and fabricated when missing, so a mistyped kind is never a free
    // pass through the enforce gate.
    if let Some(kind_word) = typed_prefix(s) {
        if NON_FILE_KINDS.contains(&kind_word.to_ascii_lowercase().as_str())
            || !text_can_be_a_path(s)
        {
            return (SourceKind::Other, s.to_string());
        }
    }
    (SourceKind::File, s.to_string())
}

/// Kinds that cite something other than a file (a command, a test run…), so
/// their text is never resolved as a path. Closed on purpose: a word outside
/// this list whose text looks like a path is judged as a file.
const NON_FILE_KINDS: &[&str] = &[
    "command",
    "cmd",
    "shell",
    "bash",
    "cli",
    "run",
    "test",
    "tests",
    "test-run",
    "output",
    "log",
    "query",
    "sql",
    "grep",
    "tool",
    "mcp",
    "benchmark",
    "measurement",
];

/// The `kind` of a `kind: text` marker: a bare identifier followed by a colon
/// and whitespace. A path (`a/b.rs:3`, `C:\x`, `a.rs: 3`) never has that
/// shape: it carries a `/`, a `.` or has no space after its colon.
fn typed_prefix(s: &str) -> Option<&str> {
    let (head, rest) = s.split_once(':')?;
    (rest.starts_with(char::is_whitespace)
        && !rest.trim().is_empty()
        && head.starts_with(|c: char| c.is_ascii_alphabetic())
        && head.len() > 1
        && head
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then_some(head)
}

/// Whether the text after an unknown `kind:` could name a file: it holds a
/// path separator, a file extension or a `:line` spec, or it is a single
/// whitespace-free token (`Makefile`). Prose with spaces is not a path.
fn text_can_be_a_path(s: &str) -> bool {
    let text = s.split_once(':').map_or(s, |(_, t)| t).trim();
    let single_token = !text.contains(char::is_whitespace);
    let (path, lines) = split_path_and_lines(text);
    text.contains('/')
        || text.contains('\\')
        || single_token
        || lines.is_some()
        || std::path::Path::new(path)
            .extension()
            .is_some_and(|e| !e.is_empty())
}

/// The cited path and line span of a `file:` or `code-comment:` marker,
/// cleaned exactly as the verifier cleans it. `None` for any other kind.
pub fn file_citation(raw: &str) -> Option<(String, Option<(usize, usize)>)> {
    file_citation_anchored(raw).map(|(path, lines, _)| (path, lines))
}

/// [`file_citation`] plus the `#symbol` anchor that may follow the line spec.
pub fn file_citation_anchored(raw: &str) -> Option<FileCitation> {
    let (kind, reference) = classify_source(raw);
    if !matches!(kind, SourceKind::File | SourceKind::CodeComment) {
        return None;
    }
    let (reference, anchor) = split_anchor(clean_reference(&reference));
    let (path, lines) = split_path_and_lines(reference);
    let path = clean_reference(path);
    (!path.is_empty()).then(|| (path.to_string(), lines, anchor.map(str::to_string)))
}

/// Path, line span and symbol anchor of a file citation.
pub type FileCitation = (String, Option<(usize, usize)>, Option<String>);

/// `(manager/package, checked date)` of a `deps:` marker, the manager and the
/// package left joined: the manager label may itself hold a `/` (`JS / TS`), so
/// only the producer's snapshot can tell where one ends and the other starts.
/// The date starts after the last `@`, since a package name may hold `@`
/// (`@types/node`).
pub fn dep_reference(raw: &str) -> Option<(String, String)> {
    let (kind, reference) = classify_source(raw);
    if kind != SourceKind::Deps {
        return None;
    }
    let reference = clean_reference(&reference);
    let (head, date) = reference.rsplit_once('@')?;
    let (head, date) = (head.trim(), date.trim());
    if head.is_empty() || date.is_empty() {
        return None;
    }
    Some((head.to_string(), date.to_string()))
}

/// `(manager, package, checked date)` of a `deps:` marker, split at the first
/// `/`. Right for a bare manager (`npm`, `cargo`) and enough to tell a
/// well-formed marker from a partial one; a label holding a `/` is split
/// wrongly, so a check against the watch uses [`dep_reference`].
pub fn dep_citation(raw: &str) -> Option<(String, String, String)> {
    let (head, date) = dep_reference(raw)?;
    let (manager, package) = head.split_once('/')?;
    let (manager, package) = (manager.trim(), package.trim());
    if manager.is_empty() || package.is_empty() {
        return None;
    }
    Some((manager.to_string(), package.to_string(), date))
}

/// The address a `url:` marker (or a bare `https://…` one) cites, without the
/// `url:` keyword or any trailing comment. `None` for any other kind, or an
/// empty reference.
pub fn url_reference(raw: &str) -> Option<String> {
    let (kind, reference) = classify_source(raw);
    if kind != SourceKind::Url {
        return None;
    }
    let reference = clean_reference(&reference);
    let lower = reference.to_ascii_lowercase();
    let start = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| lower.find(scheme))
        .min()
        .unwrap_or(0);
    let url = clean_reference(reference[start..].split_whitespace().next()?);
    (!url.is_empty()).then(|| url.to_string())
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
pub fn split_path_and_lines(reference: &str) -> (&str, Option<(usize, usize)>) {
    if let Some(idx) = reference.rfind(':') {
        let (path, after) = (&reference[..idx], &reference[idx + 1..]);
        if !after.is_empty() {
            if let Some((s, e)) = parse_line_spec(after) {
                return (path, Some((s, e)));
            }
            // A presence citation names the file, not a line of it.
            if is_presence_marker(after) {
                return (path, None);
            }
        }
    }
    (reference, None)
}

/// Suffixes of a presence citation, `[src: file: <path>:exists]`: the claim is
/// that the file is there (or tracked), so it needs no line and never invites
/// quoting the content of a private key or a log.
const PRESENCE_MARKERS: &[&str] = &["exists", "tracked"];

/// Whether the text after the last `:` of a file citation is a presence marker.
pub fn is_presence_marker(after: &str) -> bool {
    let after = after.trim();
    PRESENCE_MARKERS
        .iter()
        .any(|marker| after.eq_ignore_ascii_case(marker))
}

/// Whether the presence marker claims the file is tracked by git, which the
/// index must confirm: `exists` is settled by the file being there.
pub fn is_tracked_marker(after: &str) -> bool {
    after.trim().eq_ignore_ascii_case("tracked")
}

/// A `path:tracked` citation that no checkout root's git index backs.
fn untracked_claim(reference: &str, roots: &[&Path]) -> bool {
    let Some((path, after)) = clean_reference(reference).rsplit_once(':') else {
        return false;
    };
    is_tracked_marker(after) && !roots.iter().any(|root| is_git_tracked(root, path.trim()))
}

/// Whether git tracks `rel` in the repository at `root`. Outside a repository,
/// or without git, nothing is tracked.
pub fn is_git_tracked(root: &Path, rel: &str) -> bool {
    agent_git_succeeds(
        root,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            rel,
        ],
    )
}

/// A git query on a checkout an agent may have reconfigured: no program its
/// configuration names runs, and a blocking configuration (a FIFO) fails the
/// query in bounded time (`false`).
fn agent_git_succeeds(root: &Path, args: &[&str]) -> bool {
    crate::core::cmd::agent_git_output(root, args, &[]).is_some()
}

/// Whether git ignores `rel` in the repository at `root` (a vendored or generated
/// path, absent from a clone), whether or not the file is on disk. Outside a
/// repository, or without git, nothing is ignored.
pub fn is_git_ignored(root: &Path, rel: &str) -> bool {
    agent_git_succeeds(root, &["check-ignore", "-q", "--", rel])
}

/// The en and em dash, which models write where a line range wants a hyphen.
pub const RANGE_DASHES: [char; 2] = ['\u{2013}', '\u{2014}'];

/// A line spec with its en/em dashes turned into the hyphen they stand for. The
/// one place every gate (verifier, citation drift, documentary gate) reads a
/// range through, so none of them disagrees on what a range is.
pub fn normalize_range_dashes(s: &str) -> String {
    s.replace(RANGE_DASHES, "-")
}

/// `12` or `12-20` as written, without judging the numbers: `None` when a bound
/// is missing or not a number, so a caller can say "not a line number".
pub fn parse_line_bounds(s: &str) -> Option<(usize, usize)> {
    let normalized = normalize_range_dashes(s);
    let bounds: Vec<_> = normalized
        .split('-')
        .map(|bound| bound.trim().parse::<usize>())
        .collect();
    match bounds.as_slice() {
        [Ok(line)] => Some((*line, *line)),
        [Ok(start), Ok(end)] => Some((*start, *end)),
        _ => None,
    }
}

fn parse_line_spec(s: &str) -> Option<(usize, usize)> {
    parse_line_bounds(s).filter(|&(start, end)| start != 0 && end >= start)
}

/// Peel a `#symbol` anchor off `path:line#symbol` / `path:start-end#symbol`.
/// Only after a line spec: a `#` elsewhere stays part of the path.
fn split_anchor(reference: &str) -> (&str, Option<&str>) {
    let Some((head, symbol)) = reference.rsplit_once('#') else {
        return (reference, None);
    };
    let head = head.trim_end();
    let symbol = symbol.trim();
    if is_anchor_symbol(symbol) && split_path_and_lines(head).1.is_some() {
        (head, Some(symbol))
    } else {
        (reference, None)
    }
}

/// An identifier, optionally dotted or dashed (`fn_name`, `Type.method`, `check-version`).
pub fn is_anchor_symbol(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '.' | '-'))
        && !s.ends_with(['.', '-'])
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// `symbol` appears in `line` as a whole word.
fn contains_word(line: &str, symbol: &str) -> bool {
    line.match_indices(symbol).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + symbol.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

/// Why a `#symbol` anchor does not hold for its range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorMiss {
    /// What the message reports: the definitions of the symbol inside the
    /// range when one of them fails to open it, else every line holding the
    /// symbol. Possibly none.
    pub lines: Vec<usize>,
    /// Where a corrected range would start for each of `lines`: the top of
    /// the doc comment and attributes of a definition, the line itself for a
    /// reference.
    pub starts: Vec<usize>,
    /// The range contains a definition of the symbol that does not open it.
    pub definition: bool,
    /// The file defines the symbol somewhere. `lines` then names definitions
    /// only, never call sites: the anchor is the source of truth, so a single
    /// one is where the citation belongs, however far it moved.
    pub defined: bool,
}

/// Sibling lines (a constant and its comment, say) a range may open with
/// before the prelude of the definition it anchors on.
const ANCHOR_SIBLING_LINES: usize = 5;

/// How far above a definition its doc comment and attributes are searched.
const PRELUDE_SEARCH_LINES: usize = 80;

/// Words that open a definition: the name after one of them is defined there.
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn",
    "def",
    "class",
    "struct",
    "enum",
    "const",
    "interface",
    "function",
    "type",
];

/// Tokens that may sit between the start of a line and its definition keyword.
const DEFINITION_MODIFIERS: &[&str] = &[
    "pub", "async", "unsafe", "extern", "export", "default", "declare", "abstract",
];

/// `line` defines `symbol`: a definition keyword, after any visibility or
/// `async`-like modifier, then the symbol as the whole name. Comments and prose
/// never match.
fn defines_symbol(line: &str, symbol: &str) -> bool {
    let mut tokens = line.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        // `pub(in crate::api)`: the space after `in` splits the path.
        if token.starts_with("pub(") && !token.contains(')') {
            for next in tokens.by_ref() {
                if next.contains(')') {
                    break;
                }
            }
            continue;
        }
        let modifier = DEFINITION_MODIFIERS.contains(&token)
            || token.starts_with("pub(")
            || token.starts_with('"')
            || (token == "const"
                && tokens
                    .peek()
                    .is_some_and(|next| matches!(*next, "fn" | "async" | "unsafe" | "extern")));
        if modifier {
            continue;
        }
        // `function* run` (generator): the star may hug the keyword or the name.
        if !DEFINITION_KEYWORDS.contains(&token.trim_end_matches('*')) {
            return false;
        }
        let name = tokens.next().unwrap_or("").trim_start_matches('*');
        let end = name.find(|c: char| !is_word_char(c)).unwrap_or(name.len());
        return !symbol.is_empty() && &name[..end] == symbol;
    }
    false
}

/// Every line of `lines` is a doc comment or an attribute, so a definition
/// right below them still opens the range. A multi-line attribute counts until
/// its brackets close; brackets in a comment never count.
fn is_definition_prelude(lines: &[&str]) -> bool {
    let mut depth = 0i32;
    let mut in_block_comment = false;
    for line in lines {
        let text = line.trim_start();
        if in_block_comment {
            in_block_comment = !text.contains("*/");
            continue;
        }
        let attribute = text.starts_with("#[") || text.starts_with("#![") || text.starts_with('@');
        if depth > 0 || attribute {
            let opened = text.matches(['(', '[', '{']).count() as i32;
            let closed = text.matches([')', ']', '}']).count() as i32;
            depth = (depth + opened - closed).max(0);
        } else if text.starts_with("/*") {
            in_block_comment = !text.contains("*/");
        } else if !(text.starts_with("//") || text.starts_with('*') || text.starts_with('#')) {
            return false;
        }
    }
    depth == 0 && !in_block_comment
}

/// The first line of the doc comment and attributes sitting right above the
/// definition at `line` (1-based), else `line` itself.
fn prelude_start(text: &[&str], line: usize) -> usize {
    // A prelude is a handful of lines: stop looking well before the file start.
    (line.saturating_sub(PRELUDE_SEARCH_LINES).max(1)..line)
        .find(|&first| is_definition_prelude(&text[first - 1..line - 1]))
        .unwrap_or(line)
}

/// `None` when the anchor holds (or cannot be checked), else why not.
///
/// A range that contains no definition of the symbol (see [`defines_symbol`])
/// only has to hold the word: a call site, a field or a heading backs a claim
/// about wiring. A range that contains a definition must open with it, a doc
/// comment or attributes allowed above and up to [`ANCHOR_SIBLING_LINES`]
/// sibling lines before them: a range that starts a whole function early
/// drifts from the function it names.
pub fn anchor_miss(path: &Path, span: (usize, usize), symbol: &str) -> Option<AnchorMiss> {
    if crate::acp::secret_files::is_secret_path(path) {
        return None; // never read: the range itself is refused upstream
    }
    if std::fs::metadata(path).ok()?.len() > LINE_COUNT_SIZE_CAP_BYTES {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    anchor_miss_in(&content, span, symbol)
}

/// [`anchor_miss`] on the text of a file, such as a past revision's blob.
pub fn anchor_miss_in(content: &str, span: (usize, usize), symbol: &str) -> Option<AnchorMiss> {
    if content.len() as u64 > LINE_COUNT_SIZE_CAP_BYTES {
        return None;
    }
    let text: Vec<&str> = content.lines().collect();
    let (start, end) = span;
    let holding: Vec<usize> = text
        .iter()
        .enumerate()
        .filter(|(_, line)| contains_word(line, symbol))
        .map(|(index, _)| index + 1)
        .collect();
    let in_range = |line: &usize| (start..=end).contains(line);
    let is_defined = |line: &usize| defines_symbol(text[line - 1], symbol);
    let definitions: Vec<usize> = holding.iter().copied().filter(is_defined).collect();
    let defined_here: Vec<usize> = holding
        .iter()
        .copied()
        .filter(|l| in_range(l) && is_defined(l))
        .collect();
    if defined_here.is_empty() {
        if holding.iter().any(in_range) {
            return None;
        }
        // A symbol the file defines is placed by its definitions: its call
        // sites are not where a citation of it belongs.
        let lines = if definitions.is_empty() {
            holding
        } else {
            definitions.clone()
        };
        let starts = lines
            .iter()
            .map(|&l| {
                if is_defined(&l) {
                    prelude_start(&text, l)
                } else {
                    l
                }
            })
            .collect();
        return Some(AnchorMiss {
            lines,
            starts,
            definition: false,
            defined: !definitions.is_empty(),
        });
    }
    let opens_range = defined_here.iter().any(|&line| {
        (start.max(1)..=line.min(start + ANCHOR_SIBLING_LINES))
            .any(|from| is_definition_prelude(&text[from - 1..line - 1]))
    });
    if opens_range {
        return None;
    }
    let starts = defined_here
        .iter()
        .map(|&l| prelude_start(&text, l))
        .collect();
    Some(AnchorMiss {
        lines: defined_here,
        starts,
        definition: true,
        defined: true,
    })
}

/// [`line_bounds_status`], then the anchor: a symbol absent from the cited
/// lines makes the range wrong, so it reads as out of bounds.
pub fn anchored_bounds_status(
    candidate: &Path,
    line_spec: Option<(usize, usize)>,
    anchor: Option<&str>,
) -> (SourceStatus, String) {
    let verdict = line_bounds_status(candidate, line_spec);
    let (Some(span), Some(symbol), SourceStatus::Verified) = (line_spec, anchor, verdict.0) else {
        return verdict;
    };
    match anchor_miss(candidate, span, symbol) {
        None => verdict,
        Some(miss) => (
            SourceStatus::OutOfBounds,
            anchor_miss_detail(span, symbol, &miss),
        ),
    }
}

pub fn anchor_miss_detail(span: (usize, usize), symbol: &str, miss: &AnchorMiss) -> String {
    let (start, end) = span;
    let range = if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}-{end}")
    };
    let lines = &miss.lines;
    if lines.is_empty() {
        return format!("symbol `{symbol}` not in {range}, nor anywhere in the file");
    }
    let shown: Vec<String> = lines.iter().take(5).map(usize::to_string).collect();
    let noun = if lines.len() == 1 { "line" } else { "lines" };
    if !miss.definition {
        let verb = if miss.defined {
            "it is defined at"
        } else {
            "it appears at"
        };
        return format!(
            "symbol `{symbol}` not in {range}; {verb} {noun} {}",
            shown.join(", ")
        );
    }
    format!(
        "symbol `{symbol}` is defined at {noun} {}, not at the start of {range}: a definition anchor must start the range",
        shown.join(", ")
    )
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

/// Bytes sniffed to tell a binary file from a text one.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// A file with no lines to cite (.docx, .pdf, images), told apart by content,
/// not extension: a NUL byte, invalid UTF-8, or a PDF header in the first
/// bytes. Such a file is cited by bare path, never `path:line`.
pub fn is_binary_file(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(BINARY_SNIFF_BYTES);
    if file
        .take(BINARY_SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    is_binary_content(&head)
}

/// [`is_binary_file`] on content already read: only its first
/// [`BINARY_SNIFF_BYTES`] are judged.
pub fn is_binary_content(content: &[u8]) -> bool {
    let head = &content[..content.len().min(BINARY_SNIFF_BYTES)];
    if head.starts_with(b"%PDF-") || head.contains(&0) {
        return true;
    }
    // A multi-byte character cut by the sniff window is not invalid UTF-8.
    std::str::from_utf8(head).is_err_and(|e| e.error_len().is_some())
}

/// Percent-decoded relative form of a cited or linked path (`My%20File.docx`),
/// or `None` when decoding changes nothing or yields a rooted path. The caller
/// still jails the result like any other relative path.
pub fn percent_decoded_relative(raw: &str) -> Option<String> {
    let decoded = crate::core::message_file_links::percent_decode(raw)?;
    let path = Path::new(&decoded);
    let rooted = path.has_root()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::Prefix(_)));
    (decoded != raw && !decoded.contains('\0') && !rooted).then_some(decoded)
}

/// The last line a citation may end on. A newline-terminated file of N lines
/// reads as N+1 numbered lines in an editor or in Claude Code's Read tool (the
/// last one empty), so a model that copies the number it saw cites N+1: that is
/// accepted, and it is the one place every range check takes its end bound from.
pub fn last_citable_line(content: &str) -> usize {
    content.lines().count() + usize::from(content.ends_with('\n'))
}

/// Whether `start..=end` may be cited in a file of `lines` real lines whose last
/// citable line is `last` ([`last_citable_line`]). The range has to start on a
/// real line — one that starts on the empty line N+1 cites nothing — and may end
/// on N+1. Every range check goes through here, so the verifier, the documentary
/// gate, citation drift and the bundle repair cannot disagree.
pub fn range_is_citable(lines: usize, last: usize, start: usize, end: usize) -> bool {
    start >= 1 && start <= lines && start <= end && end <= last
}

/// `(real line count, last citable line)` of a file, `None` when it is too big
/// to bounds-check. An empty file has no line: both are 0.
fn count_lines_capped(path: &Path) -> Option<(usize, usize)> {
    if crate::acp::secret_files::is_secret_path(path) {
        return None; // a secret file is never read for its lines
    }
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > LINE_COUNT_SIZE_CAP_BYTES {
        return None; // too big — skip the bounds check, existence is enough
    }
    let content = std::fs::read_to_string(path).ok()?;
    Some((content.lines().count(), last_citable_line(&content)))
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
    verify_source_marker_lanes(raw, roots, &[])
}

/// [`verify_source_marker_roots`], then the project's own worktrees for a file
/// the roots do not hold. The verdict carries which tree held it.
pub fn verify_source_marker_lanes(raw: &str, roots: &[&Path], lanes: &[LaneRoot]) -> SourceCheck {
    let (kind, reference) = classify_source(raw);
    let mut origin = None;

    let status_detail = match kind {
        SourceKind::Url | SourceKind::Api => (
            SourceStatus::Unchecked,
            "URL/API — not network-checked (SSRF-safe)".to_string(),
        ),
        SourceKind::Commit => (
            SourceStatus::Unchecked,
            "commit ref — not verified".to_string(),
        ),
        SourceKind::Deps => (
            SourceStatus::Unchecked,
            "dependency watch row — verified only against an audit's snapshot".to_string(),
        ),
        SourceKind::User => (
            SourceStatus::Unchecked,
            "user-declared — not machine-verifiable".to_string(),
        ),
        SourceKind::Inferred => (
            SourceStatus::Unchecked,
            "inferred — a derived guess, not a verifiable fact".to_string(),
        ),
        SourceKind::Hypothesis => (
            SourceStatus::Unchecked,
            "hypothesis — unverified, confirm before acting".to_string(),
        ),
        SourceKind::TrainingData => (
            SourceStatus::Rejected,
            "training-data is not a citable source — refused".to_string(),
        ),
        SourceKind::Other => (
            SourceStatus::Unchecked,
            "unknown source type — not verified".to_string(),
        ),
        // A code comment still gets existence-verified, but is flagged LOW trust.
        SourceKind::CodeComment => {
            let (st, detail, found_in) = verify_file_ref_located(&reference, roots, lanes);
            origin = found_in;
            (
                st,
                format!("code comment (not authoritative — verify): {}", detail),
            )
        }
        SourceKind::File => {
            let (st, detail, found_in) = verify_file_ref_located(&reference, roots, lanes);
            let untracked = st == SourceStatus::Verified
                && found_in.as_ref().is_some_and(SourceOrigin::is_checkout)
                && untracked_claim(&reference, roots);
            origin = found_in;
            if untracked {
                (
                    SourceStatus::NotFound,
                    format!("{detail} — but git does not track it"),
                )
            } else {
                (st, detail)
            }
        }
    };

    SourceCheck {
        raw: raw.trim().to_string(),
        kind,
        status: status_detail.0,
        detail: status_detail.1,
        origin,
    }
}

/// Strip the wrappers agents routinely put around an inline path citation:
/// surrounding backticks / quotes / angle / round brackets, and trailing
/// sentence punctuation. Pure string hygiene — it never ADDS path components,
/// so it can't widen the path-escape (SSRF) surface. This is the fix for the
/// "plein de fichiers non trouvés" bug: a perfectly valid `` `src/foo.rs:42` ``
/// used to fail because the backticks blocked both the line-spec peel and the
/// existence check.
pub fn clean_reference(reference: &str) -> &str {
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

/// The verdict for a line or range cited into a secret file.
fn secret_line_verdict() -> (SourceStatus, String) {
    (
        SourceStatus::Rejected,
        "this file may hold secrets, so no line of it is cited — state that it exists \
         with `path:exists` and do not quote its content"
            .into(),
    )
}

/// Refusal for a line or range cited into a secret file, judged on the written
/// path (`rel` under `root`) AND on `resolved`, the file the jail settled on:
/// `.env -> settings-data` is secret by its name although its target is not.
/// `None` for presence-only citations and ordinary files.
pub fn secret_line_refusal(
    root: &Path,
    rel: &Path,
    resolved: &Path,
    line_spec: Option<(usize, usize)>,
) -> Option<(SourceStatus, String)> {
    line_spec?;
    let secret = crate::acp::secret_files::is_secret_file(rel)
        || crate::acp::secret_files::is_secret_path(&root.join(rel))
        || crate::acp::secret_files::is_secret_path(resolved);
    secret.then(secret_line_verdict)
}

/// Line-bounds verdict for an existing candidate file.
pub fn line_bounds_status(
    candidate: &Path,
    line_spec: Option<(usize, usize)>,
) -> (SourceStatus, String) {
    match line_spec {
        None => (SourceStatus::Verified, "file exists".into()),
        // The gate every line check ends in (verifier, documentary gate, step
        // lint): a line of a secret file, written or reached through a symlink,
        // is a pointer into its content. Only presence may be cited. Callers that
        // hold the written path check it first (`secret_line_refusal`), since
        // `candidate` is already resolved.
        Some(_) if crate::acp::secret_files::is_secret_path(candidate) => secret_line_verdict(),
        Some(_) if is_binary_file(candidate) => (
            SourceStatus::OutOfBounds,
            "binary file has no lines — cite it by bare path, without a line".into(),
        ),
        Some((start, end)) => match count_lines_capped(candidate) {
            None => (
                SourceStatus::Verified,
                "file exists (too large to bounds-check lines)".into(),
            ),
            Some((total, citable)) => {
                if range_is_citable(total, citable, start, end) {
                    (
                        SourceStatus::Verified,
                        format!("lines {}-{} within {} lines", start, end, total),
                    )
                } else {
                    // A range that starts on the empty line N+1 is as much
                    // beyond the file as one that ends past it.
                    let beyond = if start > total { start } else { end };
                    (
                        SourceStatus::OutOfBounds,
                        format!("line {} beyond file length {}", beyond, total),
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
    "node_modules",
    "vendor",
    "target",
    ".git",
    "dist",
    "build",
    ".next",
    ".kronn",
    ".kronn-worktrees",
    ".venv",
    "__pycache__",
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
        let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
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
pub fn path_has_segment_suffix(full: &str, needle: &str) -> bool {
    if full == needle {
        return true;
    }
    full.strip_suffix(needle)
        .is_some_and(|rest| rest.ends_with('/'))
}

/// Resolve a MULTI-SEGMENT relative citation to a UNIQUE file by segment-aligned
/// full-path suffix match. Fixes the dominant inline-anchor false positive: a
/// path cited relative to an app SUBDIR (project code lives under `application/`
/// but the verify root is the project root → `apps/x/Foo.php` is really at
/// `application/apps/x/Foo.php`), or a sibling-repo path written with the repo
/// name (`front_node/plugins/…`, resolved once that linked repo is a root).
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
        let walker = walkdir::WalkDir::new(root).into_iter().filter_entry(|e| {
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

/// Outcome of resolving a relative path inside one root.
pub enum Jailed {
    /// The real, symlink-free path of an existing entry inside the root.
    Inside(PathBuf),
    Missing,
    /// A `..` or a symlink leads out of the root.
    Escapes,
}

/// Cap on symlinks followed while resolving one citation (loops included).
const MAX_SYMLINK_HOPS: usize = 40;

/// Resolve `rel` under `root` one component at a time with lstat/readlink
/// only, so an escaping symlink is refused before its target is ever stat'ed:
/// the verdict reveals nothing about files outside the root.
pub fn resolve_in_root(root: &Path, rel: &Path) -> Jailed {
    use std::collections::VecDeque;
    use std::ffi::OsString;
    use std::path::Component;
    /// Queue `path`'s components in front of `queue`; `false` on a rooted one.
    fn push_front(queue: &mut VecDeque<OsString>, path: &Path) -> bool {
        for comp in path.components().rev() {
            match comp {
                Component::CurDir => {}
                Component::Prefix(_) | Component::RootDir => return false,
                c => queue.push_front(c.as_os_str().to_os_string()),
            }
        }
        true
    }
    let Ok(canon_root) = root.canonicalize() else {
        return Jailed::Missing;
    };
    let bases = [canon_root.clone(), normalize_lexical(root)];
    let mut queue = VecDeque::new();
    if !push_front(&mut queue, rel) {
        return Jailed::Escapes;
    }
    let mut current = canon_root.clone();
    let mut hops = 0usize;
    while let Some(comp) = queue.pop_front() {
        if comp == ".." {
            if current == canon_root {
                return Jailed::Escapes;
            }
            current.pop();
            continue;
        }
        let next = current.join(&comp);
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            return Jailed::Missing;
        };
        if !meta.file_type().is_symlink() {
            current = next;
            continue;
        }
        hops += 1;
        if hops > MAX_SYMLINK_HOPS {
            return Jailed::Missing;
        }
        let Ok(target) = std::fs::read_link(&next) else {
            return Jailed::Missing;
        };
        let relative = if target.has_root() {
            // `..` stays in the remainder: the walk resolves it physically.
            let Some(inside) = bases.iter().find_map(|b| target.strip_prefix(b).ok()) else {
                return Jailed::Escapes;
            };
            current = canon_root.clone();
            inside.to_path_buf()
        } else {
            target
        };
        if !push_front(&mut queue, &relative) {
            return Jailed::Escapes;
        }
    }
    Jailed::Inside(current)
}

/// Lexical forms of an absolute citation: as written, and mapped onto the
/// Docker host-home mount. Never touches the filesystem.
fn absolute_forms(path_str: &str) -> Vec<PathBuf> {
    let mut forms = vec![normalize_lexical(Path::new(path_str))];
    for alias in crate::core::scanner::host_home_aliases() {
        if let Some(rest) = path_str.strip_prefix(alias.as_str()) {
            if rest.is_empty() || rest.starts_with('/') {
                forms.push(normalize_lexical(Path::new(&format!("/host-home{rest}"))));
            }
        }
    }
    forms
}

/// The root holding an absolute citation, and the path relative to it.
fn absolute_under_root<'a>(path_str: &str, roots: &[&'a Path]) -> Option<(&'a Path, PathBuf)> {
    let forms = absolute_forms(path_str);
    roots.iter().find_map(|root| {
        let mut bases = vec![normalize_lexical(root)];
        if let Ok(canon) = root.canonicalize() {
            bases.push(canon);
        }
        forms.iter().find_map(|form| {
            bases
                .iter()
                .find_map(|base| form.strip_prefix(base).ok())
                .map(|rest| (*root, rest.to_path_buf()))
        })
    })
}

/// Verify a file reference against one or more candidate roots. The FIRST root
/// where the (jailed) relative path exists wins — this is how an Isolated
/// discussion's git worktree is tried before the main checkout, fixing the
/// false-NotFound on files the agent saw/created in the worktree.
///
/// - Absolute path → accepted only when it lies under a root (lexical match),
///   then jailed like a relative path; anywhere else → OutsideProject.
/// - Relative path → lexical jail + symlink-aware walk under EACH root.
///   `../` escape in every root → OutsideProject; jailed-but-absent everywhere
///   → NotFound. Nothing outside a root is stat'ed (no existence oracle).
/// - Bare basename (no separator) unresolved at root level → unique-match walk
///   (`resolve_unique_basename`) before NotFound.
pub fn verify_file_ref(reference: &str, roots: &[&Path]) -> (SourceStatus, String) {
    verify_file_ref_walk(reference, roots, true)
}

/// `walk: false` skips the basename / suffix tree walks (direct paths only).
fn verify_file_ref_walk(reference: &str, roots: &[&Path], walk: bool) -> (SourceStatus, String) {
    let verdict = verify_file_ref_as_written(reference, roots, walk);
    if verdict.0 != SourceStatus::NotFound {
        return verdict;
    }
    // `My%20File.docx` names `My File.docx`; the decoded form is relative only.
    match percent_decoded_relative(clean_reference(reference)) {
        Some(decoded) => match verify_file_ref_as_written(&decoded, roots, walk) {
            decoded_verdict if decoded_verdict.0 != SourceStatus::NotFound => decoded_verdict,
            _ => verdict,
        },
        None => verdict,
    }
}

fn verify_file_ref_as_written(
    reference: &str,
    roots: &[&Path],
    walk: bool,
) -> (SourceStatus, String) {
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

    let (reference, anchor) = split_anchor(reference);
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

    // Absolute → accepted only under a root, then jailed like a relative path.
    // Matched lexically: nothing outside the roots is ever stat'ed.
    if path.has_root() || path.is_absolute() {
        let Some((root, rel)) = absolute_under_root(path_str, roots) else {
            return (
                SourceStatus::OutsideProject,
                "absolute path outside the project root".into(),
            );
        };
        return match resolve_in_root(root, &rel) {
            Jailed::Inside(found) => secret_line_refusal(root, &rel, &found, line_spec)
                .unwrap_or_else(|| anchored_bounds_status(&found, line_spec, anchor)),
            Jailed::Missing => (
                SourceStatus::NotFound,
                format!("file not found: {}", path_str),
            ),
            Jailed::Escapes => (
                SourceStatus::OutsideProject,
                "resolves (via symlink) outside the project root".into(),
            ),
        };
    }

    // Relative → try each root. Lexical jail, then a symlink-aware walk.
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
        match resolve_in_root(root, path) {
            Jailed::Inside(found) => {
                return secret_line_refusal(root, path, &found, line_spec)
                    .unwrap_or_else(|| anchored_bounds_status(&found, line_spec, anchor))
            }
            Jailed::Missing => continue,
            Jailed::Escapes => {
                return (
                    SourceStatus::OutsideProject,
                    "resolves (via symlink) outside the project root".into(),
                )
            }
        }
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
    if !walk {
        // Direct resolution only.
    } else if !path_str.is_empty() && !path_str.contains('/') && !path_str.contains('\\') {
        match resolve_unique_basename(path_str, roots) {
            BasenameResolution::Unique(found) => {
                let (status, detail) = anchored_bounds_status(&found, line_spec, anchor);
                let shown = roots
                    .iter()
                    .find_map(|r| found.strip_prefix(r).ok())
                    .unwrap_or(found.as_path())
                    .display();
                return (
                    status,
                    format!("{detail} — resolved by unique basename → {shown}"),
                );
            }
            BasenameResolution::Ambiguous(n) => {
                return (
                    SourceStatus::NotFound,
                    format!(
                        "bare name `{path_str}` matches {n} files — ambiguous, cite the full path"
                    ),
                );
            }
            BasenameResolution::NotFound => {}
        }
    } else {
        // Multi-segment relative path unresolved at root level — it may live
        // deeper (cited relative to an app subdir, e.g. `apps/x/Foo.php` actually
        // at `application/apps/x/Foo.php`) or in a sibling repo by name
        // (`front_node/plugins/…`). Walk for a UNIQUE segment-aligned suffix
        // match across the roots (incl. linked_repos) before giving up.
        match resolve_unique_path_suffix(path_str, roots) {
            BasenameResolution::Unique(found) => {
                let (status, detail) = anchored_bounds_status(&found, line_spec, anchor);
                let shown = roots
                    .iter()
                    .find_map(|r| found.strip_prefix(r).ok())
                    .unwrap_or(found.as_path())
                    .display();
                return (
                    status,
                    format!("{detail} — resolved by unique path suffix → {shown}"),
                );
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

    (
        SourceStatus::NotFound,
        format!("file not found: {}", path_str),
    )
}

/// Cap on the worktrees scanned for an unresolved citation (one stat each,
/// at lint finalisation). Lanes come most recent first, so the cap drops the
/// stale ones; an explicit `.kronn/worktrees/<lane>/` path ignores it.
const MAX_LANE_ROOTS: usize = 256;

/// Resolve a file citation and say which tree held it. Order, first match
/// wins: an explicit `.kronn/worktrees/<lane>/` (or absolute lane) path goes to
/// that lane only; otherwise the checkout roots are tried first, and the lanes
/// only for a path the checkout lacks (or holds with fewer lines). Lanes use
/// direct paths only (no tree walk) and the same jail and line checks.
fn verify_file_ref_located(
    reference: &str,
    roots: &[&Path],
    lanes: &[LaneRoot],
) -> (SourceStatus, String, Option<SourceOrigin>) {
    let cleaned = clean_reference(reference);
    if let Some((lane, rest)) = explicit_lane_ref(cleaned, lanes) {
        let (status, detail) = verify_file_ref_walk(&rest, &[lane.path.as_path()], false);
        if located(status) {
            let detail = format!("{detail} — in worktree {}", lane.name);
            return (status, detail, Some(SourceOrigin::worktree(&lane.name)));
        }
        return (
            status,
            format!("{detail} — looked in worktree {}", lane.name),
            None,
        );
    }
    if let Some(detail) = foreign_worktree_path(cleaned, roots) {
        return (SourceStatus::OutsideProject, detail, None);
    }

    let (status, detail) = verify_file_ref(reference, roots);
    let checkout_origin = located(status).then(SourceOrigin::checkout);
    if lanes.is_empty() || !matches!(status, SourceStatus::NotFound | SourceStatus::OutOfBounds) {
        return (status, detail, checkout_origin);
    }
    let mut lane_out_of_bounds: Option<(String, &LaneRoot)> = None;
    for lane in lanes.iter().take(MAX_LANE_ROOTS) {
        let (lane_status, lane_detail) =
            verify_file_ref_walk(reference, &[lane.path.as_path()], false);
        match lane_status {
            SourceStatus::Verified => {
                let note = if status == SourceStatus::OutOfBounds {
                    "the checkout copy is shorter"
                } else {
                    "not in the checkout"
                };
                return (
                    SourceStatus::Verified,
                    format!("{lane_detail} — found in worktree {} ({note})", lane.name),
                    Some(SourceOrigin::worktree(&lane.name)),
                );
            }
            SourceStatus::OutOfBounds if lane_out_of_bounds.is_none() => {
                lane_out_of_bounds = Some((lane_detail, lane));
            }
            _ => {}
        }
    }
    if status == SourceStatus::NotFound {
        if let Some((lane_detail, lane)) = lane_out_of_bounds {
            return (
                SourceStatus::OutOfBounds,
                format!(
                    "{lane_detail} — in worktree {}, not in the checkout",
                    lane.name
                ),
                Some(SourceOrigin::worktree(&lane.name)),
            );
        }
        let n = lanes.len().min(MAX_LANE_ROOTS);
        return (
            status,
            format!("{detail} — not in the checkout nor in the project's {n} active worktree(s)"),
            None,
        );
    }
    (status, detail, checkout_origin)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => normalize_lexical(a) == normalize_lexical(b),
    }
}

/// A verdict that means the file was found (even if the lines were not).
fn located(status: SourceStatus) -> bool {
    matches!(status, SourceStatus::Verified | SourceStatus::OutOfBounds)
}

/// `.kronn/worktrees/<lane>/rest[:lines]`, or an absolute path under a lane,
/// rewritten as `rest[:lines]` relative to that lane.
fn explicit_lane_ref<'a>(cleaned: &str, lanes: &'a [LaneRoot]) -> Option<(&'a LaneRoot, String)> {
    let relative = cleaned.strip_prefix("./").unwrap_or(cleaned);
    if let Some(tail) = relative.strip_prefix(".kronn/worktrees/") {
        let (name, rest) = tail.split_once('/')?;
        let lane = lanes.iter().find(|lane| lane.name == name)?;
        return (!rest.is_empty()).then(|| (lane, rest.to_string()));
    }
    let (path_part, line_spec) = split_path_and_lines(cleaned);
    let path_part = clean_reference(path_part);
    if !Path::new(path_part).is_absolute() {
        return None;
    }
    let forms = absolute_forms(path_part);
    lanes.iter().find_map(|lane| {
        let base = normalize_lexical(&lane.path);
        let rest = forms.iter().find_map(|f| f.strip_prefix(&base).ok())?;
        let rest = rest.to_str().filter(|r| !r.is_empty())?;
        let lines = match line_spec {
            Some((start, end)) if start == end => format!(":{start}"),
            Some((start, end)) => format!(":{start}-{end}"),
            None => String::new(),
        };
        Some((lane, format!("{rest}{lines}")))
    })
}

/// An absolute path inside some `.kronn/worktrees/` that is neither one of
/// this project's lanes nor under its roots belongs to another project.
fn foreign_worktree_path(cleaned: &str, roots: &[&Path]) -> Option<String> {
    let (path_part, _) = split_path_and_lines(cleaned);
    let path_part = clean_reference(path_part);
    if !Path::new(path_part).is_absolute() {
        return None;
    }
    let forms = absolute_forms(path_part);
    let in_worktrees = forms.iter().any(|form| {
        let names: Vec<&std::ffi::OsStr> = form.iter().collect();
        names
            .windows(2)
            .any(|pair| pair[0] == ".kronn" && pair[1] == "worktrees")
    });
    let under_root = absolute_under_root(path_part, roots).is_some();
    (in_worktrees && !under_root)
        .then(|| format!("{path_part} is in a worktree of another project — refused"))
}

/// The `.git` file a worktree registration (`<repo>/.git/worktrees/<name>/`)
/// points at. Git may store that path relative to the registration directory
/// (`../../../.kronn/worktrees/task-kt-9/.git`): such a path is joined to its
/// directory and normalized here, not handed to `resolve_host_path`, which
/// refuses every `..` and would log a warning per registered worktree. An
/// absolute path is the host path it always was.
fn registered_worktree_dot_git(registration: &Path, recorded: &str) -> PathBuf {
    let recorded = Path::new(recorded);
    if recorded.is_absolute() {
        return crate::core::scanner::resolve_host_path(&recorded.to_string_lossy());
    }
    let mut normalized = PathBuf::new();
    for component in registration.join(recorded).components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// The worktrees of the project checked out at `checkout`: every directory
/// under its `.kronn/worktrees/`, plus the worktrees git registers for it
/// (`.git/worktrees/*/gitdir`). Read from disk, no subprocess; symlinked
/// entries are not lanes. Most recently modified first, uncapped: callers
/// cap the scan, never the lookup of a lane named explicitly.
pub fn project_lane_roots(checkout: &Path) -> Vec<LaneRoot> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(checkout.join(".kronn").join("worktrees")) {
        paths.extend(entries.filter_map(|e| e.ok()).map(|e| e.path()));
    }
    if let Ok(entries) = std::fs::read_dir(checkout.join(".git").join("worktrees")) {
        for entry in entries.filter_map(|e| e.ok()) {
            let Ok(gitdir) = std::fs::read_to_string(entry.path().join("gitdir")) else {
                continue;
            };
            let dot_git = registered_worktree_dot_git(&entry.path(), gitdir.trim());
            if let Some(worktree) = dot_git.parent() {
                paths.push(worktree.to_path_buf());
            }
        }
    }
    let checkout_canon = checkout.canonicalize().ok();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut lanes: Vec<(std::time::SystemTime, LaneRoot)> = Vec::new();
    for path in paths {
        // No follow: a symlink under `.kronn/worktrees` may point anywhere.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_dir() {
            continue;
        }
        let canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        if checkout_canon.as_ref() == Some(&canon) || seen.contains(&canon) {
            continue;
        }
        seen.push(canon);
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            let name = name.to_string();
            lanes.push((modified, LaneRoot { name, path }));
        }
    }
    lanes.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    lanes.into_iter().map(|(_, lane)| lane).collect()
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
pub fn extract_inline_file_anchors(text: &str) -> Vec<String> {
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

pub fn looks_like_file_anchor(s: &str) -> bool {
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
    analyze_roots_lanes(text, roots, &[])
}

/// [`analyze_roots`] that also resolves file citations in the project's own
/// worktrees (`lanes`), after the roots. A lane-found file is not fabricated.
pub fn analyze_roots_lanes(text: &str, roots: &[&Path], lanes: &[LaneRoot]) -> LintReport {
    let mut report = lint_assertions(text);
    let mut fabricated = 0u32;
    let mut unverified = 0u32;
    let mut sources = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for raw in extract_source_markers(text)
        .into_iter()
        .take(MAX_SOURCES_VERIFIED)
    {
        let check = verify_source_marker_lanes(&raw, roots, lanes);
        if check.status.is_fabricated() {
            fabricated += 1;
        }
        // A kind Kronn cannot check is surfaced as unverifiable, never silent.
        if check.kind == SourceKind::Other {
            unverified += 1;
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
    // Quoted markers are verified like the others, but a failure counts as
    // unverified, not fabricated: backticks also quote the grammar as an example.
    for raw in extract_quoted_source_markers(text)
        .into_iter()
        .take(MAX_SOURCES_VERIFIED)
    {
        let is_file = matches!(
            classify_source(&raw).0,
            SourceKind::File | SourceKind::CodeComment
        );
        if is_file && !seen.insert(file_path_key(&raw)) {
            continue;
        }
        let mut check = verify_source_marker_lanes(&raw, roots, lanes);
        if check.status.is_fabricated() {
            unverified += 1;
            check.status = SourceStatus::Unchecked;
            check.detail = format!("quoted in backticks (couldn't verify): {}", check.detail);
            check.origin = None;
        }
        sources.push(check);
    }
    // Niveau 1.5 — natural backticked path anchors. Resolve → verified (green);
    // don't resolve → surfaced honestly as "unverified" (soft amber), NOT red.
    if !roots.is_empty() {
        for anchor in extract_inline_file_anchors(text)
            .into_iter()
            .take(MAX_SOURCES_VERIFIED)
        {
            let key = file_path_key(&anchor);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            let (status, detail, origin) = verify_file_ref_located(&anchor, roots, lanes);
            if status == SourceStatus::Verified {
                sources.push(SourceCheck {
                    raw: anchor,
                    kind: SourceKind::File,
                    status,
                    detail: format!("inline anchor (auto-verified): {}", detail),
                    origin,
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
                    origin: None,
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
    let checkout =
        (!project_path.is_empty()).then(|| crate::core::scanner::resolve_host_path(project_path));
    let mut lanes = checkout
        .as_deref()
        .map(project_lane_roots)
        .unwrap_or_default();
    if let Some(wp) = workspace_path.filter(|w| !w.is_empty()) {
        let workspace = crate::core::scanner::resolve_host_path(wp);
        // The discussion's own worktree is a lane of the project: still tried
        // first among lanes, but after the checkout, so a merged file reads
        // "checkout". A workspace outside the project keeps its root slot.
        match lanes
            .iter()
            .position(|lane| same_dir(&lane.path, &workspace))
        {
            Some(index) => {
                let lane = lanes.remove(index);
                lanes.insert(0, lane);
            }
            None if checkout.as_deref().is_some_and(|c| same_dir(c, &workspace)) => {}
            None => roots.push(workspace),
        }
    }
    if let Some(checkout) = checkout {
        roots.push(checkout);
    }
    // 0.8.8 — also resolve citations against the project's declared linked_repos
    // (filesystem locations only). Triage/dev agents legitimately cite sibling
    // repos they were told about (front_node, …); without these as roots a
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
    let report = analyze_roots_lanes(text, &root_refs, &lanes);
    tracing::info!(
        target: "anti_halluc",
        unsourced = report.unsourced_count,
        fabricated = report.fabricated_count,
        verified = report.verified_count(),
        roots = roots.len(),
        lanes = lanes.len(),
        "lint finalized",
    );
    if report.has_signal() {
        Some(report)
    } else {
        None
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    /// KT-1595 round 12: a cited path is taken literally: a glob never counts
    /// as a tracked file.
    #[test]
    fn a_glob_is_never_a_tracked_path() {
        let dir = crate::core::cmd::hostile_git::repo();
        assert!(is_git_tracked(dir.path(), "a.txt"));
        assert!(!is_git_tracked(dir.path(), "*"));
        assert!(!is_git_tracked(dir.path(), "a.*"));
    }

    #[test]
    fn truncate_outside_markers_never_cuts_a_marker() {
        let text = "aaaa [src: file: src/lib.rs:10] bbbb [src: file: x[0].rs:1] cccc";
        for max in 1..=text.chars().count() + 2 {
            for ellipsis in [false, true] {
                let out = truncate_outside_markers(text, max, ellipsis);
                assert!(out.chars().count() <= max, "max {max}: {out}");
                let opened = out.to_ascii_lowercase().matches("[src:").count();
                assert_eq!(
                    opened,
                    extract_source_markers(&out).len(),
                    "max {max} ellipsis {ellipsis}: {out}"
                );
            }
        }
        // Under the limit: untouched. Cut before a marker: stops at its `[`.
        assert_eq!(truncate_outside_markers("short", 40, true), "short");
        assert_eq!(
            truncate_outside_markers("aaaa [src: file: a.rs:1] z", 12, false),
            "aaaa "
        );
        assert_eq!(
            truncate_outside_markers("aaaa [src: file: a.rs:1] z", 12, true),
            "aaaa…"
        );
        // A marker already unclosed in the input is cut before its opener too.
        assert_eq!(
            truncate_outside_markers("ab [src: file: a", 8, false),
            "ab "
        );
        assert_eq!(
            truncate_outside_markers("ab [src: file: a.rs:1", 12, true),
            "ab…"
        );
    }

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

    #[test]
    fn scan_fences_marks_every_line_of_a_closed_block_fence_lines_included() {
        let scan = scan_fences("a\n```rust\nb\n```\nc", &['`'], false);
        assert_eq!(scan.in_fence, vec![false, true, true, true, false]);
        assert!(!scan.unclosed);
    }

    #[test]
    fn scan_fences_tilde_blocks_only_count_when_asked_for() {
        let text = "a\n~~~\nb\n~~~\nc";
        assert_eq!(
            scan_fences(text, &['`'], false).in_fence,
            vec![false; 5],
            "the prose linters stay backtick-only"
        );
        assert_eq!(
            scan_fences(text, &['`', '~'], false).in_fence,
            vec![false, true, true, true, false]
        );
        // A tilde never closes a backtick fence (nor the other way round).
        assert_eq!(
            scan_fences("```\n~~~\nx", &['`', '~'], true).in_fence,
            vec![true, true, true]
        );
        // A tilde fence's info string may carry a backtick.
        assert_eq!(
            scan_fences("~~~ a`b\nx\n~~~", &['`', '~'], false).in_fence,
            vec![true, true, true]
        );
    }

    #[test]
    fn scan_fences_unclosed_block_is_visible_or_code_on_request() {
        let text = "a\n```\nb\nc";
        let lenient = scan_fences(text, &['`'], false);
        assert!(lenient.unclosed);
        assert_eq!(lenient.in_fence, vec![false; 4], "restored as visible");
        let strict = scan_fences(text, &['`'], true);
        assert!(strict.unclosed);
        assert_eq!(strict.in_fence, vec![false, true, true, true]);
    }

    // ── Mode ──────────────────────────────────────────────────────────

    #[test]
    fn mode_parse_is_lenient_and_defaults_to_warn() {
        assert_eq!(AntiHallucMode::from_str_lenient("off"), AntiHallucMode::Off);
        assert_eq!(
            AntiHallucMode::from_str_lenient("WARN"),
            AntiHallucMode::Warn
        );
        assert_eq!(
            AntiHallucMode::from_str_lenient(" Enforce "),
            AntiHallucMode::Enforce
        );
        // Unknown / empty → warn (rollout default).
        assert_eq!(
            AntiHallucMode::from_str_lenient("bogus"),
            AntiHallucMode::Warn
        );
        assert_eq!(AntiHallucMode::from_str_lenient(""), AntiHallucMode::Warn);
    }

    #[test]
    fn mode_round_trips_and_active_flag() {
        for m in [
            AntiHallucMode::Off,
            AntiHallucMode::Warn,
            AntiHallucMode::Enforce,
        ] {
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
        assert!(should_auto_attach_doc_author(
            AntiHallucMode::Enforce,
            &none,
            true
        ));
        // Wrong mode → never.
        assert!(!should_auto_attach_doc_author(
            AntiHallucMode::Warn,
            &none,
            true
        ));
        assert!(!should_auto_attach_doc_author(
            AntiHallucMode::Off,
            &none,
            true
        ));
        // No docs/AGENTS.md → never (nothing to discipline against).
        assert!(!should_auto_attach_doc_author(
            AntiHallucMode::Enforce,
            &none,
            false
        ));
        // Idempotent: already attached → don't duplicate.
        let attached = vec!["rust".to_string(), "kronn-doc-author".to_string()];
        assert!(!should_auto_attach_doc_author(
            AntiHallucMode::Enforce,
            &attached,
            true
        ));
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
}

#[cfg(test)]
mod binary_citations {
    use super::*;

    fn project() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Budget 2026.docx"),
            b"PK\x03\x04\x14\x00\x06\x00body",
        )
        .unwrap();
        std::fs::write(root.join("notes.md"), "one\ntwo\n").unwrap();
        std::fs::write(tmp.path().join("secret.md"), "outside\n").unwrap();
        tmp
    }

    fn status(raw: &str, root: &Path) -> SourceStatus {
        verify_source_marker(raw, Some(root)).status
    }

    #[test]
    fn a_binary_file_has_no_lines_but_exists() {
        let tmp = project();
        let root = tmp.path().join("proj");
        assert_eq!(
            status("file: Budget 2026.docx", &root),
            SourceStatus::Verified
        );
        let check = verify_source_marker("file: Budget 2026.docx:1", Some(&root));
        assert_eq!(check.status, SourceStatus::OutOfBounds);
        assert!(
            check.detail.contains("cite it by bare path"),
            "{}",
            check.detail
        );
        assert_eq!(status("file: notes.md:2", &root), SourceStatus::Verified);
    }

    #[test]
    fn percent_encoded_paths_resolve_without_escaping_the_root() {
        let tmp = project();
        let root = tmp.path().join("proj");
        assert_eq!(
            status("file: Budget%202026.docx", &root),
            SourceStatus::Verified
        );
        assert_eq!(status("file: notes%2Emd:1", &root), SourceStatus::Verified);
        assert_eq!(
            status("file: ..%2Fsecret.md:1", &root),
            SourceStatus::OutsideProject
        );
        let encoded_absolute = format!(
            "file: %2F{}",
            tmp.path()
                .join("secret.md")
                .display()
                .to_string()
                .trim_start_matches('/')
        );
        assert!(status(&encoded_absolute, &root).is_fabricated());
    }
}

#[cfg(test)]
mod lane_origin {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kronn_lane_{tag}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, lines: usize) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "x\n".repeat(lines)).unwrap();
    }

    /// A checkout with one lane under `.kronn/worktrees/0151-kt1181`.
    fn project() -> (PathBuf, PathBuf) {
        let checkout = temp_dir("checkout");
        let lane = checkout.join(".kronn/worktrees/0151-kt1181");
        std::fs::create_dir_all(&lane).unwrap();
        (checkout, lane)
    }

    fn check(raw: &str, checkout: &Path) -> SourceCheck {
        let lanes = project_lane_roots(checkout);
        verify_source_marker_lanes(raw, &[checkout], &lanes)
    }

    #[test]
    fn file_in_checkout_and_lane_reads_checkout() {
        let (checkout, lane) = project();
        write(&checkout, "src/a.rs", 5);
        write(&lane, "src/a.rs", 5);
        let c = check("file: src/a.rs:3", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::checkout()));
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn lane_only_file_is_verified_in_the_named_worktree() {
        let (checkout, lane) = project();
        write(&lane, "frontend/src/lib/templateMerge.ts", 10);
        let c = check("file: frontend/src/lib/templateMerge.ts:4", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("0151-kt1181")));
        assert!(c.detail.contains("0151-kt1181"), "{}", c.detail);
        assert!(!c.status.is_fabricated());
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn file_missing_everywhere_is_not_found() {
        let (checkout, _lane) = project();
        let c = check("file: nowhere/ghost.rs:1", &checkout);
        assert_eq!(c.status, SourceStatus::NotFound);
        assert_eq!(c.origin, None);
        assert!(c.detail.contains("1 active worktree"), "{}", c.detail);
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn line_out_of_bounds_in_a_lane_file_is_out_of_bounds_there() {
        let (checkout, lane) = project();
        write(&lane, "src/new.rs", 3);
        let c = check("file: src/new.rs:9", &checkout);
        assert_eq!(c.status, SourceStatus::OutOfBounds, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("0151-kt1181")));
        assert!(c.detail.contains("beyond file length 3"), "{}", c.detail);
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn lines_past_the_checkout_copy_resolve_in_the_lane_that_has_them() {
        let (checkout, lane) = project();
        write(&checkout, "src/a.rs", 3);
        write(&lane, "src/a.rs", 20);
        let c = check("file: src/a.rs:15", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("0151-kt1181")));
        assert!(c.detail.contains("shorter"), "{}", c.detail);
        // Past the end of both copies: the checkout verdict stands.
        let c = check("file: src/a.rs:99", &checkout);
        assert_eq!(c.status, SourceStatus::OutOfBounds);
        assert_eq!(c.origin, Some(SourceOrigin::checkout()));
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn explicit_lane_prefix_resolves_in_that_lane_only() {
        let (checkout, lane) = project();
        write(&lane, "src/x.rs", 4);
        let c = check("file: .kronn/worktrees/0151-kt1181/src/x.rs:2", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("0151-kt1181")));
        let c = check("file: .kronn/worktrees/0151-kt1181/src/x.rs:7", &checkout);
        assert_eq!(c.status, SourceStatus::OutOfBounds, "{}", c.detail);
        let absolute = format!("file: {}:2", lane.join("src/x.rs").display());
        let c = check(&absolute, &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("0151-kt1181")));
        // The lane jail holds: `../` out of the lane is refused.
        let c = check(
            "file: .kronn/worktrees/0151-kt1181/../../../../etc/hosts",
            &checkout,
        );
        assert_eq!(c.status, SourceStatus::OutsideProject, "{}", c.detail);
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn another_projects_worktree_is_refused() {
        let (checkout, _lane) = project();
        let other = temp_dir("other");
        write(&other, ".kronn/worktrees/lane-x/src/f.rs", 3);
        let lanes = project_lane_roots(&checkout);
        assert!(lanes.iter().all(|l| l.name != "lane-x"), "{lanes:?}");

        let absolute = format!(
            "file: {}:1",
            other.join(".kronn/worktrees/lane-x/src/f.rs").display()
        );
        let c = check(&absolute, &checkout);
        assert_eq!(c.status, SourceStatus::OutsideProject, "{}", c.detail);
        assert_eq!(c.origin, None);
        // Not a lane of this project: the prefix names nothing here.
        let c = check("file: .kronn/worktrees/lane-x/src/f.rs:1", &checkout);
        assert_eq!(c.status, SourceStatus::NotFound, "{}", c.detail);
        std::fs::remove_dir_all(&checkout).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn git_registered_worktree_outside_the_checkout_is_a_lane() {
        let checkout = temp_dir("checkout");
        let elsewhere = temp_dir("wt");
        write(&elsewhere, "src/only_here.rs", 2);
        let meta = checkout.join(".git/worktrees/kt-9");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(
            meta.join("gitdir"),
            format!("{}\n", elsewhere.join(".git").display()),
        )
        .unwrap();
        // A registration whose tree is gone is not a lane.
        let stale = checkout.join(".git/worktrees/gone");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("gitdir"), "/nonexistent/kronn/gone/.git\n").unwrap();

        let lanes = project_lane_roots(&checkout);
        assert_eq!(lanes.len(), 1, "{lanes:?}");
        let name = lanes[0].name.clone();
        let c = verify_source_marker_lanes("file: src/only_here.rs:2", &[&checkout], &lanes);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree(&name)));
        std::fs::remove_dir_all(&checkout).ok();
        std::fs::remove_dir_all(&elsewhere).ok();
    }

    /// KT-1316 — git can record a worktree's `.git` relative to its registration;
    /// that is a lane like any other, found without a `resolve_host_path` refusal.
    #[test]
    fn a_relative_worktree_registration_is_a_lane() {
        let checkout = temp_dir("checkout");
        write(&checkout, ".kronn/worktrees/task-kt-9/src/here.rs", 2);
        let meta = checkout.join(".git/worktrees/task-kt-9");
        std::fs::create_dir_all(&meta).unwrap();
        std::fs::write(
            meta.join("gitdir"),
            "../../../.kronn/worktrees/task-kt-9/.git\n",
        )
        .unwrap();

        assert_eq!(
            registered_worktree_dot_git(&meta, "../../../.kronn/worktrees/task-kt-9/.git"),
            checkout.join(".kronn/worktrees/task-kt-9/.git")
        );
        let lanes = project_lane_roots(&checkout);
        // Found through both the directory listing and the registration: once.
        assert_eq!(lanes.len(), 1, "{lanes:?}");
        assert_eq!(lanes[0].name, "task-kt-9");
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn legacy_row_without_origin_reads_as_checkout() {
        let legacy = r#"{"unsourced_count":0,"flagged_spans":[],"sources":[
            {"raw":"file: src/a.rs:1","kind":"file","status":"verified","detail":"file exists"}
        ],"fabricated_count":0}"#;
        let report: LintReport = serde_json::from_str(legacy).unwrap();
        assert_eq!(report.sources[0].origin, None);
        assert_eq!(report.verified_count(), 1);
        // Absent stays absent on the way out: old readers see the old shape.
        let json = serde_json::to_value(&report.sources[0]).unwrap();
        assert!(json.get("origin").is_none(), "{json}");
        let lane = SourceCheck {
            origin: Some(SourceOrigin::worktree("0151-kt1181")),
            ..report.sources[0].clone()
        };
        let json = serde_json::to_value(&lane).unwrap();
        assert_eq!(
            json["origin"],
            serde_json::json!({"kind":"worktree","name":"0151-kt1181"})
        );
        let back: SourceCheck = serde_json::from_value(json).unwrap();
        assert_eq!(back, lane);
    }

    #[test]
    fn inline_anchor_in_a_lane_is_verified_not_unverified() {
        let (checkout, lane) = project();
        write(&lane, "frontend/src/lib/templateMerge.ts", 10);
        let lanes = project_lane_roots(&checkout);
        let report = analyze_roots_lanes(
            "See `frontend/src/lib/templateMerge.ts:3`.",
            &[&checkout],
            &lanes,
        );
        assert_eq!(report.unverified_count, 0, "{:?}", report.sources);
        assert_eq!(report.sources.len(), 1);
        assert_eq!(report.sources[0].status, SourceStatus::Verified);
        assert_eq!(
            report.sources[0].origin,
            Some(SourceOrigin::worktree("0151-kt1181"))
        );
        std::fs::remove_dir_all(&checkout).ok();
    }

    fn set_mtime(dir: &Path, secs_ago: u64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs_ago);
        std::fs::File::open(dir)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    /// More lanes than the scan cap: `lane-000` … plus `zz-active` (last by
    /// name, most recent) and `aa-stale` (first by name, oldest).
    fn crowded_project() -> (PathBuf, PathBuf, PathBuf) {
        let checkout = temp_dir("crowded");
        let base = checkout.join(".kronn/worktrees");
        for i in 0..MAX_LANE_ROOTS + 5 {
            let lane = base.join(format!("lane-{i:03}"));
            std::fs::create_dir_all(&lane).unwrap();
            set_mtime(&lane, 3_600 + i as u64);
        }
        let active = base.join("zz-active");
        write(&active, "src/active.rs", 4);
        set_mtime(&active, 0);
        let stale = base.join("aa-stale");
        write(&stale, "src/stale.rs", 4);
        set_mtime(&stale, 90_000);
        (checkout, active, stale)
    }

    #[test]
    fn most_recent_lane_is_scanned_even_past_the_cap_by_name() {
        let (checkout, _active, _stale) = crowded_project();
        let lanes = project_lane_roots(&checkout);
        assert_eq!(lanes.len(), MAX_LANE_ROOTS + 7);
        assert_eq!(lanes[0].name, "zz-active");
        let c = check("file: src/active.rs:2", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("zz-active")));
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[test]
    fn explicit_lane_path_resolves_its_lane_beyond_the_scan_cap() {
        let (checkout, _active, _stale) = crowded_project();
        let lanes = project_lane_roots(&checkout);
        let position = lanes.iter().position(|l| l.name == "aa-stale").unwrap();
        assert!(position >= MAX_LANE_ROOTS, "aa-stale at {position}");
        let c = check("file: .kronn/worktrees/aa-stale/src/stale.rs:3", &checkout);
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(c.origin, Some(SourceOrigin::worktree("aa-stale")));
        std::fs::remove_dir_all(&checkout).ok();
    }

    #[cfg(unix)] // creates symlinks through std::os::unix
    #[test]
    fn symlinked_lane_entry_is_ignored() {
        let (checkout, _lane) = project();
        let outside = temp_dir("outside");
        write(&outside, "src/secret.rs", 3);
        std::os::unix::fs::symlink(&outside, checkout.join(".kronn/worktrees/evil")).unwrap();
        let lanes = project_lane_roots(&checkout);
        assert!(lanes.iter().all(|l| l.name != "evil"), "{lanes:?}");
        let c = check("file: src/secret.rs:1", &checkout);
        assert_eq!(c.status, SourceStatus::NotFound, "{}", c.detail);
        assert_eq!(c.origin, None);
        // Named explicitly, it is no lane: the checkout's symlink guard refuses it.
        let c = check("file: .kronn/worktrees/evil/src/secret.rs:1", &checkout);
        assert_eq!(c.status, SourceStatus::OutsideProject, "{}", c.detail);
        std::fs::remove_dir_all(&checkout).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    // Serial: `set_mode` mutates the process-global mode.
    #[serial_test::serial]
    #[test]
    fn lint_receipt_counts_lane_files_as_found() {
        let (checkout, lane) = project();
        write(&checkout, "src/main.rs", 5);
        write(&lane, "frontend/new.ts", 5);
        let _mode = ModeOverride::set("warn");
        let text = "Done [src: file: src/main.rs:2], new [src: file: frontend/new.ts:3], \
                    and [src: file: src/ghost.rs:1].";
        let report =
            finalize_lint_report(text, None, checkout.to_str().unwrap(), &[]).expect("signal");
        assert_eq!(report.fabricated_count, 1, "{:?}", report.sources);
        assert_eq!(report.verified_count(), 2);
        let origins: Vec<_> = report.sources.iter().map(|s| s.origin.clone()).collect();
        assert_eq!(
            origins,
            vec![
                Some(SourceOrigin::checkout()),
                Some(SourceOrigin::worktree("0151-kt1181")),
                None
            ]
        );

        // The discussion's own worktree is a lane: after the checkout.
        write(&lane, "src/main.rs", 5);
        let report = finalize_lint_report(
            "[src: file: src/main.rs:2] [src: file: frontend/new.ts:1]",
            Some(lane.to_str().unwrap()),
            checkout.to_str().unwrap(),
            &[],
        )
        .expect("signal");
        assert_eq!(report.fabricated_count, 0);
        assert_eq!(report.sources[0].origin, Some(SourceOrigin::checkout()));
        assert_eq!(
            report.sources[1].origin,
            Some(SourceOrigin::worktree("0151-kt1181"))
        );
        std::fs::remove_dir_all(&checkout).ok();
    }
}

#[cfg(test)]
mod deps_citation_tests {
    use super::*;

    #[test]
    fn deps_marker_is_its_own_kind_and_unchecked_without_a_snapshot() {
        let check = verify_source_marker("deps: npm/react@2026-10-04", None);
        assert_eq!(check.kind, SourceKind::Deps);
        assert_eq!(check.status, SourceStatus::Unchecked);
        assert!(!check.status.is_fabricated());
    }

    #[test]
    fn dep_citation_splits_scoped_packages_and_rejects_partial_refs() {
        assert_eq!(
            dep_citation("deps: npm/@types/node@2026-10-04"),
            Some(("npm".into(), "@types/node".into(), "2026-10-04".into()))
        );
        assert_eq!(
            dep_citation("deps: cargo/serde@2026-10-04"),
            Some(("cargo".into(), "serde".into(), "2026-10-04".into()))
        );
        assert_eq!(dep_citation("deps: npm/react"), None);
        assert_eq!(dep_citation("deps: npm/@types/node"), None);
        assert_eq!(dep_citation("deps: react@2026-10-04"), None);
        assert_eq!(dep_citation("file: deps/npm.rs:1"), None);
    }

    #[test]
    fn a_file_named_like_the_keyword_stays_a_file() {
        let (kind, _) = classify_source("deps_lock.rs:3");
        assert_eq!(kind, SourceKind::File);
    }
}

#[cfg(test)]
mod anchor_citations {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(
            tmp.path().join("src/a.rs"),
            "use x;\nfn retry_once() {}\n// retry_once_more\nfn other() {}\nlet v = retry_once();\n",
        )
        .unwrap();
        tmp
    }

    fn check(raw: &str, root: &Path) -> SourceCheck {
        verify_source_marker(raw, Some(root))
    }

    #[test]
    fn the_anchor_is_parsed_after_a_line_spec_only() {
        assert_eq!(
            file_citation_anchored("file: src/a.rs:2-4#retry_once"),
            Some(("src/a.rs".into(), Some((2, 4)), Some("retry_once".into())))
        );
        assert_eq!(
            file_citation_anchored("src/a.rs:2#Type.method"),
            Some(("src/a.rs".into(), Some((2, 2)), Some("Type.method".into())))
        );
        assert_eq!(
            file_citation("file: src/a.rs:2#retry_once"),
            Some(("src/a.rs".into(), Some((2, 2))))
        );
        // No line spec: the `#` stays in the path. Not an identifier: no anchor.
        assert_eq!(
            file_citation_anchored("file: docs/a.md#intro"),
            Some(("docs/a.md#intro".into(), None, None))
        );
        assert_eq!(
            file_citation_anchored("file: src/a.rs:2#9x").map(|c| c.2),
            Some(None)
        );
    }

    #[test]
    fn a_symbol_inside_the_range_verifies() {
        let tmp = repo();
        let c = check("file: src/a.rs:1-2#retry_once", tmp.path());
        assert_eq!(c.status, SourceStatus::Verified, "{}", c.detail);
        assert_eq!(
            check("src/a.rs:5#retry_once", tmp.path()).status,
            SourceStatus::Verified
        );
    }

    #[test]
    fn a_symbol_outside_the_range_is_out_of_bounds_and_says_where_it_is() {
        let tmp = repo();
        // Line 3 holds `retry_once_more`, not the word `retry_once`.
        let c = check("file: src/a.rs:3-4#retry_once", tmp.path());
        assert_eq!(c.status, SourceStatus::OutOfBounds);
        assert!(c.status.is_fabricated());
        assert_eq!(
            c.detail,
            "symbol `retry_once` not in lines 3-4; it is defined at line 2"
        );
        let c = check("file: src/a.rs:1#nowhere", tmp.path());
        assert_eq!(
            c.detail,
            "symbol `nowhere` not in line 1, nor anywhere in the file"
        );
    }

    #[test]
    fn line_bounds_still_win_over_the_anchor() {
        let tmp = repo();
        let c = check("file: src/a.rs:4-9#other", tmp.path());
        assert_eq!(c.status, SourceStatus::OutOfBounds);
        assert_eq!(c.detail, "line 9 beyond file length 5");
    }

    #[test]
    fn a_word_match_needs_boundaries() {
        assert!(contains_word("fn retry_once() {}", "retry_once"));
        assert!(contains_word("x.retry_once", "retry_once"));
        assert!(!contains_word("retry_once_more", "retry_once"));
        assert!(!contains_word("pre_retry_once", "retry_once"));
        assert!(contains_word("make check-version:", "check-version"));
        assert!(contains_word("é retry_once é", "retry_once"));
    }

    fn miss(file: &str, span: (usize, usize), symbol: &str) -> Option<AnchorMiss> {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f"), file).unwrap();
        anchor_miss(&tmp.path().join("f"), span, symbol)
    }

    #[test]
    fn a_range_containing_the_definition_must_start_at_it() {
        let file = "use x;\n\nfn run() {\n    step();\n}\n";
        assert_eq!(miss(file, (3, 5), "run"), None);
        assert_eq!(miss(file, (3, 3), "run"), None);
        // Two lines above are within the sibling allowance.
        assert_eq!(miss(file, (1, 5), "run"), None);
        // Past it, the definition is merely inside the range.
        let mut long = String::from("a\n").repeat(8);
        long.push_str(file);
        let late = miss(&long, (1, 13), "run").unwrap();
        assert_eq!(
            (late.lines, late.starts, late.definition),
            (vec![11], vec![11], true)
        );
    }

    #[test]
    fn a_definition_thirty_lines_in_fails() {
        let mut file = String::new();
        for index in 0..30 {
            file.push_str(&format!("let v{index} = {index};\n"));
        }
        file.push_str("fn run() {\n    step();\n}\n");
        let late = miss(&file, (1, 33), "run").unwrap();
        assert_eq!((late.lines, late.definition), (vec![31], true));
        // 5 siblings are forgiven, the 6th is not.
        assert_eq!(miss(&file, (26, 33), "run"), None);
        assert!(miss(&file, (25, 33), "run").is_some());
    }

    #[test]
    fn a_call_site_of_a_function_the_file_defines_passes() {
        let defined = "fn run() {\n    step();\n}\nfn step() {}\n";
        // The range holds the call, not the definition: wiring is citable.
        assert_eq!(miss(defined, (1, 3), "step"), None);
        assert_eq!(miss(defined, (2, 2), "step"), None);
        assert_eq!(miss(defined, (4, 4), "step"), None);
        // A range holding neither is wrong, and the proposal opens at the doc comment.
        let none = miss("fn run() {}\n\n/// Docs.\nfn step() {}\n", (1, 1), "step").unwrap();
        assert_eq!(
            (none.lines, none.starts, none.definition),
            (vec![4], vec![3], false)
        );
    }

    #[test]
    fn a_reference_passes_when_the_file_has_no_definition() {
        let calls = "fn run() {\n    step();\n}\n";
        assert_eq!(miss(calls, (2, 2), "step"), None);
        let gone = miss(calls, (3, 3), "step").unwrap();
        assert_eq!((gone.lines, gone.starts), (vec![2], vec![2]));
    }

    #[test]
    fn doc_comments_and_attributes_may_sit_above_the_definition() {
        let file = "x\n/// Docs.\n#[derive(\n    Debug,\n)]\n#[cfg(test)]\npub struct Run;\n";
        assert_eq!(miss(file, (2, 7), "Run"), None);
        assert_eq!(miss(file, (6, 7), "Run"), None);
        let py = "x\n@cache\n# note\ndef load():\n    pass\n";
        assert_eq!(miss(py, (2, 5), "load"), None);
        let ts = "x\n/**\n * Docs.\n */\nexport default async function main() {}\n";
        assert_eq!(miss(ts, (2, 5), "main"), None);
    }

    #[test]
    fn a_constant_and_its_comment_above_the_definition_pass() {
        let file = "a\nb\nc\nd\n/// How long.\nconst WAIT: u64 = 30;\n// Then run.\nfn run() {}\n";
        // The constant and its comment sit right above `run`: siblings.
        assert_eq!(miss(file, (5, 8), "run"), None);
        assert_eq!(miss(file, (5, 8), "WAIT"), None);
        assert_eq!(miss(file, (6, 8), "WAIT"), None);
        // `d` sits above the doc comment of WAIT: one sibling line.
        assert_eq!(miss(file, (4, 6), "WAIT"), None);
    }

    #[test]
    fn an_unbalanced_bracket_in_a_comment_does_not_break_the_prelude() {
        let file =
            "x\n/// See the `foo(` call (and [this.\n// still prose )\n#[inline]\nfn run() {}\n";
        assert_eq!(miss(file, (2, 5), "run"), None);
        let lone = miss(file, (1, 1), "run").unwrap();
        assert_eq!(lone.starts, vec![2]);
        let block = "x\n/* open (\n  prose\n*/\nfn run() {}\n";
        assert_eq!(miss(block, (2, 5), "run"), None);
        let py = "x\n# note (see\n@cache\ndef load():\n    pass\n";
        assert_eq!(miss(py, (2, 5), "load"), None);
    }

    #[test]
    fn a_range_starting_at_the_prelude_is_what_a_miss_proposes() {
        let file = "a\n\n\n\n\n\n\n\n/// Docs.\n#[test]\nfn run() {}\n";
        // Too far above the definition, no definition inside: the proposal.
        let m = miss(file, (1, 2), "run").unwrap();
        assert_eq!((m.lines, m.starts), (vec![11], vec![9]));
        assert_eq!(miss(file, (9, 11), "run"), None);
    }

    #[test]
    fn definition_keywords_and_modifiers_are_recognised() {
        for (line, symbol, defined) in [
            ("pub async fn run() {}", "run", true),
            ("pub const fn run() {}", "run", true),
            ("pub(in crate::api) fn run() {}", "run", true),
            ("pub(in crate :: api) async fn run() {}", "run", true),
            ("pub(super) struct Run;", "Run", true),
            ("pub const RUN: u8 = 1;", "RUN", true),
            ("extern \"C\" fn run() {}", "run", true),
            ("    struct Run<T>(T);", "Run", true),
            ("enum Run {", "Run", true),
            ("export interface Run {", "Run", true),
            ("export function* run() {}", "run", true),
            ("async def run():", "run", true),
            ("class Run:", "Run", true),
            ("type Run = u8;", "Run", true),
            ("fn runner() {}", "run", false),
            ("let run = 1;", "run", false),
            ("// fn run() {}", "run", false),
            ("run();", "run", false),
            ("fn", "run", false),
        ] {
            assert_eq!(defines_symbol(line, symbol), defined, "{line}");
        }
    }
}

#[cfg(test)]
mod mode_override_tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every `set_mode` reference outside this file, by file. A test that
    /// writes the mode directly leaks it into concurrent and later tests.
    fn mode_writers_by_file() -> BTreeMap<String, usize> {
        let src = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join("src");
        let path_ref = concat!("anti_halluc::set_", "mode");
        let mut found = BTreeMap::new();
        for entry in walkdir::WalkDir::new(&src) {
            let entry = entry.unwrap();
            if entry.path().extension().is_none_or(|ext| ext != "rs")
                || entry.path().ends_with("core/anti_halluc.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(entry.path()).unwrap();
            let grouped = text
                .match_indices("anti_halluc::{")
                .filter(|(at, _)| {
                    let group = &text[*at..];
                    group[..group.find('}').unwrap_or(group.len())].contains("set_mode")
                })
                .count();
            let count =
                text.matches(path_ref).count() + text.matches("anti_halluc::*").count() + grouped;
            if count > 0 {
                let rel = entry.path().strip_prefix(&src).unwrap();
                found.insert(rel.to_string_lossy().replace('\\', "/"), count);
            }
        }
        found
    }

    #[test]
    fn only_mode_override_writes_the_mode_in_tests() {
        // lib.rs: the startup wiring plus two source assertions on it;
        // setup.rs: the settings handler.
        let expected = BTreeMap::from([("lib.rs".to_string(), 3), ("api/setup.rs".to_string(), 1)]);
        assert_eq!(
            mode_writers_by_file(),
            expected,
            "tests must change the anti-hallucination mode through ModeOverride"
        );
        let own = include_str!("anti_halluc.rs");
        let calls = own.matches(concat!("set_", "mode(")).count();
        assert_eq!(
            calls, 4,
            "only the definition and ModeOverride call set_mode here"
        );
    }

    // Serial like every other ModeOverride holder, so the read after the drop is not raced.
    #[serial_test::serial]
    #[test]
    fn mode_override_restores_the_default_even_when_the_test_panics() {
        let panicked = std::panic::catch_unwind(|| {
            let _mode = ModeOverride::set("enforce");
            assert_eq!(current_mode(), AntiHallucMode::Enforce);
            panic!("test failure while the override is held");
        });
        assert!(panicked.is_err());
        assert_eq!(current_mode(), AntiHallucMode::Warn);
        // The poisoned lock and the held flag are released for the next test.
        let mode = ModeOverride::set("off");
        assert_eq!(current_mode(), AntiHallucMode::Off);
        drop(mode);
        assert_eq!(current_mode(), AntiHallucMode::Warn);
    }
}
