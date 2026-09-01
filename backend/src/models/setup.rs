// Setup & Configuration — top-level config tree (`AppConfig`),
// per-section configs (server, tokens, scan, agents, model tiers),
// the API-key types, and the Setup Wizard's status types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─── App config ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub tokens: TokensConfig,
    pub scan: ScanConfig,
    pub agents: AgentsConfig,
    /// Output language used by agents when they write their replies.
    /// Separate from `ui_language` below which controls the Kronn UI locale.
    #[serde(default = "default_language")]
    pub language: String,
    /// UI language (FR/EN/ES/ZH) for the React frontend. Persisted here so a
    /// Tauri WebView2 localStorage wipe doesn't reset the user's choice
    /// every time the app updates or Windows rotates the WebView2 profile.
    /// Frontend still writes to localStorage as a fast-path + fallback when
    /// the backend is unreachable.
    #[serde(default = "default_ui_language")]
    pub ui_language: String,
    /// Persistent STT model choice (e.g. "onnx-community/whisper-tiny").
    /// None = first-launch default / user never set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stt_model: Option<String>,
    /// Persistent TTS voice choices, keyed by output language code
    /// ("fr" → "voice-id-fr", "en" → "voice-id-en", …).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string>")]
    pub tts_voices: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub disabled_agents: Vec<AgentType>,
    #[serde(default)]
    #[ts(skip)]
    pub encryption_secret: Option<String>,
    /// Secret theme unlock codes (theme_name → code). Read-only from the
    /// server — users populate this table in their local
    /// `~/.config/kronn/config.toml` to enable hidden themes for testers.
    /// The values are NEVER exported to TypeScript and NEVER returned by
    /// any endpoint — only consumed during POST /api/themes/unlock. The
    /// public bundle therefore cannot leak them to a curious user.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(skip)]
    pub secret_themes: std::collections::HashMap<String, String>,
    /// Profile IDs the operator has unlocked via a secret code. Secret
    /// builtins (e.g. "batman") are filtered out of `GET /api/profiles`
    /// when their id is not listed here — unlock adds the id and
    /// persists the config so the profile sticks across restarts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub unlocked_profiles: Vec<String>,
    /// Skill IDs for which the frontend must NOT auto-activate even when
    /// the user's message matches the skill's `auto_triggers` regexes.
    /// Read by the frontend's `detectTriggeredSkills` filter and by the
    /// Settings UI toggle. Empty by default — every skill opts in by
    /// virtue of declaring triggers, the config lets the operator opt
    /// out per-skill without editing the skill file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub disabled_auto_skills: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Custom domain for CORS and TLS (e.g. "kronn.local")
    #[serde(default)]
    pub domain: Option<String>,
    /// Bearer token for API authentication (opt-in from Settings UI)
    #[serde(default)]
    #[ts(skip)]
    pub auth_token: Option<String>,
    /// Whether auth was explicitly enabled by the user (distinguishes from migration artifacts)
    #[serde(default)]
    #[ts(skip)]
    pub auth_enabled: bool,
    /// Strict-auth opt-in: when `true`, the localhost auto-bypass is
    /// disabled and even `127.0.0.1` / Docker bridge clients must
    /// present the Bearer token. Defaults to `false` (current
    /// pragmatic self-hosted behaviour). Flipping to `true` is the
    /// hardening path for users who run multiple processes on the
    /// same host (one of which they don't trust) — e.g. shared dev
    /// VMs. Once TLS lands (TD-20260314-no-tls) we'll deprecate the
    /// bypass entirely; this flag is the early-opt-out for users who
    /// can't wait.
    #[serde(default)]
    #[ts(skip)]
    pub auth_strict_localhost: bool,
    /// 0.8.11 (B6) — optional webhook (Slack/Teams/generic JSON) fired when a
    /// scheduled/triggered run ends in a non-success terminal state
    /// (Failed / Interrupted / StoppedByGuard). Lets an autonomous cron that
    /// dies at 6am surface immediately instead of being discovered by opening
    /// the UI. Empty/None = no notification. Also settable via
    /// `KRONN_FAILURE_NOTIFY_URL`.
    #[serde(default)]
    pub failure_notify_url: Option<String>,
    /// 0.8.11 (B7) — auto-purge workflow runs older than N days at boot.
    /// `0` (default) = DISABLED: never delete run history automatically (a fast
    /// cron's run table is 76% of the DB, but silently dropping the user's
    /// history is worse than size). Set to e.g. 90 to bound growth; parent runs
    /// still referenced by a retained child are always preserved.
    #[serde(default)]
    pub run_retention_days: u32,
    /// Encrypted execution-variable snapshot retention. `0` keeps metadata
    /// but disables value retention. Product default: 30 days.
    #[serde(default = "default_execution_variable_retention_days")]
    pub execution_variable_retention_days: u32,
    /// KT-373 — refuse to provision a worktree below this much free disk, in
    /// GiB. On 2026-08-21 the dev volume hit 100% with seven worktrees each
    /// holding its own Rust `target/`; provisioning kept going until nothing
    /// worked. A build that is refused costs a message, a build that fills the
    /// disk costs the machine.
    #[serde(default = "default_disk_critical_gib")]
    pub disk_critical_gib: u64,
    /// Warn — but still provision — below this much free disk, in GiB. Must
    /// stay above `disk_critical_gib` to mean anything; see
    /// `disk_thresholds()`, which is the only place they are read together.
    #[serde(default = "default_disk_warning_gib")]
    pub disk_warning_gib: u64,
    /// Maximum concurrent agent processes (default: 5)
    #[serde(default = "default_max_agents")]
    pub max_concurrent_agents: usize,
    /// Agent stall timeout in minutes — abort if no output for this long (default: 5)
    #[serde(default = "default_agent_stall_timeout")]
    pub agent_stall_timeout_min: u32,
    /// Absolute wall-clock limit for one agent execution, even while it keeps
    /// producing output (default: 30). This is distinct from the inactivity
    /// watchdog above and is read when each new run starts.
    #[serde(default = "default_agent_global_timeout")]
    pub agent_global_timeout_min: u32,
    /// Absolute wall-clock limit for locally-served HTTP agents (Ollama).
    /// Local inference is legitimately much slower than a hosted CLI, so it
    /// has an explicit, visible budget instead of a hidden runtime multiplier.
    #[serde(default = "default_local_agent_global_timeout")]
    pub local_agent_global_timeout_min: u32,
    /// KT-405 — per-model context override, persisted (unlike
    /// `KRONN_OLLAMA_NUM_CTX_CAP`, which is process-global and disappears on
    /// restart). Keyed by the exact Ollama model tag (`"qwen3.8:27b-mlx"`).
    /// The env var still wins when set — it is the break-glass escape hatch
    /// for an operator who cannot reach the UI; this is the persistent,
    /// per-model dial meant to be set once and forgotten. Bounds and warnings
    /// against the model's advertised window / this machine's RAM ceiling are
    /// enforced at the setter, not here — a deserialized config from an older
    /// Kronn or a smaller machine must still load.
    #[serde(default)]
    pub ollama_context_overrides: std::collections::HashMap<String, u64>,
    /// User identity — displayed in messages and used for future multi-user
    #[serde(default)]
    pub pseudo: Option<String>,
    /// Email for Gravatar avatar (optional, decoupled from git)
    #[serde(default)]
    pub avatar_email: Option<String>,
    /// Short bio — who the user is, their role, expertise. Injected at the start of first message in a discussion.
    #[serde(default)]
    pub bio: Option<String>,
    /// Global context injected into discussions. Markdown content — glossary,
    /// company conventions, stack overview, etc. Supplements project-level
    /// `ai/` context. Stored in config.toml.
    #[serde(default)]
    pub global_context: Option<String>,
    /// When to inject global_context:
    /// - `"always"` (default) — every discussion
    /// - `"no_project"` — only discussions without a project
    /// - `"never"` — disabled
    #[serde(default = "default_global_context_mode")]
    pub global_context_mode: String,
    /// 0.8.7 anti-hallucination mode: `off` | `warn` | `enforce`.
    ///
    /// - `off` — feature disabled, nothing injected or linted.
    /// - `warn` (default) — P1 sourcing directive injected + P2 lint (heuristic + mechanical `[src:]` verification) surfaced as a non-blocking pill.
    /// - `enforce` — same as `warn` in 0.8.7; reserved for the Phase 3 write-time refusal of unverifiable citations.
    ///
    /// See `core::anti_halluc`. Mirrored into the process-global flag at load + save.
    #[serde(default = "default_anti_hallucination_mode")]
    pub anti_hallucination_mode: String,
    /// 0.10.0 — Continual Learning master toggle. **Default OFF (beta)**: the
    /// feature writes agent-proposed learnings into injected truth files
    /// (`docs/learnings.md` / user-context), so it ships opt-in to avoid a bug
    /// polluting a user's docs. Gates capture (`learning_propose`), the
    /// `kronn:section name="learnings"` doc pointer, and the UI badge/modal.
    /// Validating/rejecting EXISTING pending candidates stays allowed when off
    /// (drain, don't capture). See docs/research/continual-learning-0.10.0-spec.md §0.
    #[serde(default)]
    pub continual_learning_enabled: bool,
    /// Show the per-message out-of-context note control in discussion
    /// composers. Enabled by default; turning it off hides the authoring
    /// affordance without deleting existing notes.
    #[serde(default = "default_true")]
    pub discussion_notes_enabled: bool,
    /// Debug mode — when true, the tracing subscriber is initialized at
    /// `debug` level instead of `info`, producing significantly more
    /// output on stdout. Lets users diagnose agent detection / project
    /// scan issues themselves without needing to set `RUST_LOG` by hand.
    /// Persisted in config.toml so it survives restarts. Toggleable from
    /// the Settings UI or via `./kronn start --debug` (CLI flag wins for
    /// the duration of that run).
    #[serde(default)]
    pub debug_mode: bool,
    /// 0.8.6 phase 4 — default model tier applied to NEW creations
    /// (discussions, QP drafts, workflow Agent steps) when the user
    /// doesn't explicitly pick one in the form. STRICT semantic :
    /// only consulted by creation flows on `componentDidMount` ; never
    /// applied retroactively to existing items at execution time
    /// (otherwise a user flipping the default to `Reasoning` would
    /// silently 10x the cost of every legacy QP they launch).
    ///
    /// Persisted in `config.toml`. Defaults to `Default` for
    /// backwards-compat — existing configs without the field keep
    /// the prior hardcoded behaviour.
    #[serde(default)]
    pub default_model_tier: ModelTier,
    /// 0.8.6 phase 4 — default summary strategy applied to NEW
    /// discussions. Flipped from `Auto` to `Off` because most modern
    /// agents (Claude Code, Codex, Gemini-Pro) have large context
    /// windows AND can pull older history on-demand via the
    /// `disc_load_other` MCP tool — auto-summary just burns Economy
    /// tokens for no win in those cases. The `Off` default makes
    /// Kronn cheaper out of the box.
    ///
    /// Re-enable `Auto` (Settings) for small-context or HTTP agents when a
    /// proactive summary is preferable. HTTP agents have no arbitrary MCP
    /// bridge, but a discussion run may receive bounded Kronn-native history
    /// tools; the chosen strategy remains an operator trade-off.
    ///
    /// Strict semantic — only consulted on NEW disc creation. Existing
    /// discs keep their saved value (no retroactive change).
    #[serde(default = "default_summary_strategy_off")]
    pub default_summary_strategy: crate::models::SummaryStrategy,
    /// Allow an agent's final prose to explicitly hand work to another
    /// attached agent through a canonical `@alias`. Opt-in because one
    /// generated reply can otherwise start additional paid runs.
    #[serde(default)]
    pub agent_handoffs_enabled: bool,
    /// Maximum paid or cost-unknown handoffs spawned from one originating
    /// human turn. Ollama is local and uses a separate fixed safety ceiling.
    #[serde(default = "default_agent_handoff_paid_limit")]
    pub agent_handoff_paid_limit: u32,
    /// Remove the paid/unknown per-turn quota while keeping the structural
    /// loop guards (attached agents only and bounded delegation depth).
    #[serde(default)]
    pub agent_handoff_paid_unlimited: bool,
    /// Agents that cannot be started automatically from another agent's
    /// generated reply. Empty keeps the historical allow-all behaviour.
    #[serde(default)]
    pub agent_handoff_blocked_agents: Vec<AgentType>,
    /// Sidebar storage-weight indicator. Validation and fallback live in
    /// `models::discussion_weight`; this is only the persisted field.
    #[serde(default)]
    pub discussion_weight: crate::models::DiscussionWeightConfig,
}

/// Serde default for [`ServerConfig::default_summary_strategy`].
/// Returns `Off` so a missing field in config.toml means "auto-summary
/// disabled" — the new safer default shipped 0.8.6 phase 4.
fn default_summary_strategy_off() -> crate::models::SummaryStrategy {
    crate::models::SummaryStrategy::Off
}

fn default_agent_handoff_paid_limit() -> u32 {
    1
}

fn default_global_context_mode() -> String {
    "always".to_string()
}
fn default_anti_hallucination_mode() -> String {
    crate::core::anti_halluc::DEFAULT_MODE_STR.to_string()
}
/// A cold `cargo build` of this workspace writes a few GiB; the floor is set
/// so a refused provisioning still leaves room to work, investigate and clean.
pub(crate) const DEFAULT_DISK_CRITICAL_GIB: u64 = 5;
pub(crate) const DEFAULT_DISK_WARNING_GIB: u64 = 20;
fn default_disk_critical_gib() -> u64 {
    DEFAULT_DISK_CRITICAL_GIB
}

fn default_execution_variable_retention_days() -> u32 {
    30
}
fn default_disk_warning_gib() -> u64 {
    DEFAULT_DISK_WARNING_GIB
}
fn default_max_agents() -> usize {
    5
}
fn default_agent_stall_timeout() -> u32 {
    5
}
// 240, raised from 120 (KT-403): a hard task on a locally-served 27B measured
// at ~2 h 40 of honest work. The old ceiling existed to stop hung runs from
// squatting the agent semaphore; since an abandoned tool loop now halts itself
// at the next round boundary, a long window no longer keeps a dead run alive.
pub(crate) const MAX_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 240;
pub(crate) const DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 30;
fn default_agent_global_timeout() -> u32 {
    DEFAULT_AGENT_GLOBAL_TIMEOUT_MIN
}

pub(crate) const DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN: u32 = 240;
fn default_local_agent_global_timeout() -> u32 {
    DEFAULT_LOCAL_AGENT_GLOBAL_TIMEOUT_MIN
}

pub(crate) fn clamp_agent_global_timeout_min(input: u64) -> u32 {
    input.clamp(1, u64::from(MAX_AGENT_GLOBAL_TIMEOUT_MIN)) as u32
}

/// Default output language. Used by `AppConfig.language` AND by API
/// request types deserialized from the frontend (where the user may
/// omit the field). `pub(crate)` so other model sub-modules can keep
/// the `default = "..."` attribute working after extraction.
pub(crate) fn default_language() -> String {
    "fr".into()
}
pub(crate) fn default_ui_language() -> String {
    "fr".into()
}

// ─── Tokens / API keys ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokensConfig {
    /// Legacy fields — kept for backward compat when reading old config.toml
    #[serde(default, skip_serializing)]
    pub anthropic: Option<String>,
    #[serde(default, skip_serializing)]
    pub openai: Option<String>,
    #[serde(default, skip_serializing)]
    pub google: Option<String>,
    /// All API keys (new multi-key system)
    #[serde(default)]
    pub keys: Vec<ApiKey>,
    #[serde(default)]
    pub disabled_overrides: Vec<String>,
}

impl TokensConfig {
    /// Get the active key value for a provider, or None
    pub fn active_key_for(&self, provider: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|k| k.provider == provider && k.active)
            .map(|k| k.value.as_str())
    }

    /// Whether provider credential metadata declares an active key, without
    /// returning or inspecting the secret value itself.
    pub fn has_active_key_for(&self, provider: &str) -> bool {
        self.keys
            .iter()
            .any(|key| key.provider == provider && key.active)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[ts(skip)]
    pub value: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiKeyDisplay {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub masked_value: String,
    pub active: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SaveApiKeyRequest {
    pub id: Option<String>,
    pub name: String,
    pub provider: String,
    pub value: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ApiKeysResponse {
    pub keys: Vec<ApiKeyDisplay>,
    pub disabled_overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoveredKey {
    pub provider: String,
    pub source: String,
    pub suggested_name: String,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverKeysResponse {
    pub discovered: Vec<DiscoveredKey>,
    pub imported_count: u32,
}

// ─── Scan ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScanConfig {
    pub paths: Vec<String>,
    pub ignore: Vec<String>,
    /// Max depth when scanning for git repos (2–10, default 4)
    #[serde(default = "default_scan_depth")]
    pub scan_depth: usize,
}

fn default_scan_depth() -> usize {
    4
}

// ─── Agents ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsConfig {
    pub claude_code: AgentConfig,
    pub codex: AgentConfig,
    #[serde(default)]
    pub open_code: AgentConfig,
    #[serde(default)]
    pub gemini_cli: AgentConfig,
    #[serde(default)]
    pub kiro: AgentConfig,
    #[serde(default)]
    pub vibe: AgentConfig,
    #[serde(default)]
    pub copilot_cli: AgentConfig,
    #[serde(default)]
    pub ollama: AgentConfig,
    #[serde(default)]
    pub lite_llm: AgentConfig,
    #[serde(default)]
    pub nvidia: AgentConfig,
    /// Per-agent model tier overrides (Economy/Reasoning model names).
    #[serde(default)]
    pub model_tiers: ModelTiersConfig,
}

/// Endpoint slots of the OpenAI-wire providers, carried as one value.
///
/// Both slots travel together deliberately. While they were two independent
/// fields on the spawn config, every call site wired LiteLLM's and none wired
/// NVIDIA's, so a configured NVIDIA endpoint never reached the runner and the
/// public default silently won (KT-337). One value means a call site cannot
/// wire one provider and forget the other.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpEndpoints {
    pub lite_llm: Option<String>,
    pub nvidia: Option<String>,
}

impl HttpEndpoints {
    pub fn from_agents(agents: &AgentsConfig) -> Self {
        Self {
            lite_llm: agents.lite_llm.base_url.clone(),
            nvidia: agents.nvidia.base_url.clone(),
        }
    }

    /// The endpoint slot this agent reads, `None` meaning "fall back to the
    /// provider's own default".
    pub fn for_agent(&self, agent_type: &AgentType) -> Option<&str> {
        // Deliberately EXHAUSTIVE, no catch-all. The previous `_ =>` arm handed
        // LiteLLM's proxy to every other agent, so a new OpenAI-wire provider
        // (OpenRouter, Together, …) would have inherited someone else's endpoint
        // silently — the comment warned about it without preventing it. Listing
        // every variant means the next provider fails to COMPILE here and has to
        // choose its slot, which is the only guard that cannot be forgotten.
        match agent_type {
            AgentType::Nvidia => self.nvidia.as_deref(),
            AgentType::LiteLlm => self.lite_llm.as_deref(),
            // Ollama shares the HTTP chat path but this production config has
            // no Ollama endpoint slot: the runner therefore falls back to
            // OLLAMA_HOST/Docker/localhost. CLI agents never reach that path.
            // `None` is the honest answer for both, not a fallback.
            AgentType::Ollama
            | AgentType::ClaudeCode
            | AgentType::Codex
            | AgentType::OpenCode
            | AgentType::Vibe
            | AgentType::GeminiCli
            | AgentType::Kiro
            | AgentType::CopilotCli
            | AgentType::Custom => None,
        }
    }
}

impl AgentsConfig {
    /// Get the full_access setting for a given agent type.
    pub fn full_access_for(&self, agent: &AgentType) -> bool {
        match agent {
            AgentType::ClaudeCode => self.claude_code.full_access,
            AgentType::Codex => self.codex.full_access,
            AgentType::OpenCode => self.open_code.full_access,
            AgentType::GeminiCli => self.gemini_cli.full_access,
            AgentType::Kiro => self.kiro.full_access,
            AgentType::Vibe => self.vibe.full_access,
            AgentType::CopilotCli => self.copilot_cli.full_access,
            AgentType::Ollama => self.ollama.full_access,
            AgentType::LiteLlm => self.lite_llm.full_access,
            AgentType::Nvidia => self.nvidia.full_access,
            _ => false,
        }
    }

    pub fn any_full_access(&self) -> bool {
        self.claude_code.full_access
            || self.codex.full_access
            || self.open_code.full_access
            || self.gemini_cli.full_access
            || self.kiro.full_access
            || self.vibe.full_access
            || self.copilot_cli.full_access
            || self.ollama.full_access
            || self.lite_llm.full_access
            || self.nvidia.full_access
    }

    /// Returns true if at least one agent is marked as installed.
    pub fn any_installed(&self) -> bool {
        self.claude_code.installed
            || self.codex.installed
            || self.open_code.installed
            || self.gemini_cli.installed
            || self.kiro.installed
            || self.vibe.installed
            || self.copilot_cli.installed
            || self.ollama.installed
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentConfig {
    pub path: Option<String>,
    #[serde(default)]
    pub installed: bool,
    pub version: Option<String>,
    #[serde(default)]
    pub full_access: bool,
    /// How many runs of THIS agent may execute at once. `None` takes the
    /// built-in default for its family. A remote HTTP provider parallelises
    /// fine and is left unlimited; a local one is not — Ollama serves a single
    /// inference slot, so extra runs only queue and throw away its KV cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u32>,
    /// Optional UI color used for this agent's canonical `@mention`.
    /// `None` keeps the built-in frontend color.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mention_color: Option<String>,
    /// Where to reach an agent that is a server rather than a binary. Only
    /// LiteLLM uses it today: its proxy can live anywhere, so the endpoint is
    /// the user's to declare. The matching credential lives in `TokensConfig`
    /// under the `litellm` provider, never here — this struct is serialised
    /// to the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

// ─── Model tiers ──────────────────────────────────────────────────────────

/// Abstract model capability tier. Kronn maps each tier to a concrete --model flag per agent.
/// Priority: AgentSettings.model (explicit) > ModelTier > Default (no flag).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ModelTier {
    /// Cheap/fast model (haiku, gpt-4.1-mini, flash). For summaries, bulk ops.
    Economy,
    /// Agent's built-in default. No --model flag passed.
    #[default]
    Default,
    /// Most capable model (opus, o4-mini, pro). For audits, complex analysis.
    Reasoning,
}

/// Per-agent model tier configuration. Maps Economy/Reasoning to concrete model names.
/// Stored in config.toml under [agents.model_tiers].
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
#[ts(export)]
pub struct ModelTierConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub economy: Option<String>,
    /// User override for the `Default` tier — when set, takes precedence
    /// over the built-in fallback in `resolve_model_flag`. Lets the user
    /// pick e.g. their preferred Ollama model from the OllamaCard picker
    /// without having to edit config.toml. `None` = built-in default
    /// applies, preserving backward compatibility for users who never
    /// touched the setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

/// Global model tier overrides per agent.
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
#[ts(export)]
pub struct ModelTiersConfig {
    #[serde(default)]
    pub claude_code: ModelTierConfig,
    #[serde(default)]
    pub codex: ModelTierConfig,
    #[serde(default)]
    pub open_code: ModelTierConfig,
    #[serde(default)]
    pub gemini_cli: ModelTierConfig,
    #[serde(default)]
    pub kiro: ModelTierConfig,
    #[serde(default)]
    pub vibe: ModelTierConfig,
    #[serde(default)]
    pub copilot_cli: ModelTierConfig,
    #[serde(default)]
    pub ollama: ModelTierConfig,
    #[serde(default)]
    pub lite_llm: ModelTierConfig,
    #[serde(default)]
    pub nvidia: ModelTierConfig,
}

// ─── Setup wizard ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    pub is_first_run: bool,
    pub current_step: SetupStep,
    pub agents_detected: Vec<AgentDetection>,
    pub scan_paths_set: bool,
    pub repos_detected: Vec<super::DetectedRepo>,
    pub default_scan_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SetupStep {
    Agents,
    ScanPaths,
    Detection,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentDetection {
    pub name: String,
    pub agent_type: AgentType,
    pub installed: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub latest_version: Option<String>,
    pub origin: String,
    pub install_command: Option<String>,
    #[serde(default)]
    pub host_managed: bool,
    #[serde(default)]
    pub host_label: Option<String>,
    /// Agent is runnable via npx/uvx fallback even when no local binary is found
    #[serde(default)]
    pub runtime_available: bool,
    /// Whether Kronn has the authentication material required by this runner.
    /// `None` is the backward-compatible wire value for older detection paths;
    /// API responses produced by the current backend always set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auth_ready: Option<bool>,
    /// Local command that prepares authentication when `auth_ready == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auth_setup_command: Option<String>,
    /// `rtk` binary found on the host (PATH). Same value for every agent
    /// detection in a given sweep, but kept per-agent so the frontend can
    /// render the state inline without a separate endpoint.
    #[serde(default)]
    pub rtk_available: bool,
    /// The agent's own config file declares an RTK hook. Always `false` for
    /// agents that have no shell-exec (API-only agents like Vibe) or no
    /// hookable config (Ollama) — they're considered non-applicable.
    #[serde(default)]
    pub rtk_hook_configured: bool,
    /// Optional i18n key for a runtime-degradation warning the frontend
    /// should surface inline. Set per-agent at detect time.
    /// Examples:
    ///   - `"vibe.sdk_fallback"` — Vibe SDK signature mismatch detected
    ///     (sentinel file present); the runner falls back to direct API
    ///     mode, losing the local-tools (bash/file I/O) capability.
    ///   - `"vibe.project_config_untrusted"` — Vibe's workspace-trust store
    ///     rejects a `.vibe` directory Kronn manages, so it loads none of
    ///     the MCP servers Kronn wrote there.
    ///
    /// `None` means "no degradation detected, agent is healthy".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_warning: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[ts(export)]
pub enum AgentType {
    /// 0.8.5 — picked as the serde default for `WorkflowStep.agent`. The
    /// field is required at runtime for agent-driven steps (Agent /
    /// BatchQuickPrompt) but irrelevant for non-LLM steps (ApiCall,
    /// Exec, Gate, …). Before this default the wizard had to invent a
    /// placeholder agent on every ApiCall step or the JSON payload
    /// failed to deserialize on `PUT /workflow-steps/test-api-call`
    /// with `missing field "agent"` (caught the user during the JIRA
    /// helper dogfooding on 2026-05-17). ClaudeCode is the safe pick
    /// because it's the only agent guaranteed to be installed by the
    /// onboarding flow.
    #[default]
    ClaudeCode,
    Codex,
    /// OpenCode uses the ACP transport; it is a CLI identity, never the
    /// generic `Custom` HTTP connection bucket.
    OpenCode,
    Vibe,
    GeminiCli,
    Kiro,
    CopilotCli,
    /// Local LLM via Ollama (0.4.0). Runs over the HTTP `/api/chat` path,
    /// not a CLI process. It has no host shell or arbitrary MCP bridge; Kronn
    /// exposes a bounded native workspace/Git/API catalogue server-side when
    /// the execution context provides the corresponding scope.
    Ollama,
    /// OpenAI-compatible proxy (LiteLLM). Same HTTP execution path as Ollama,
    /// different wire format (`OpenAiCodec`). "Installed" means the `litellm`
    /// binary is present; "reachable" means the proxy is actually running,
    /// which the health endpoint reports separately.
    LiteLlm,
    /// NVIDIA-hosted models (0.11.0). Same OpenAI-compatible HTTP path as
    /// LiteLLM — one API key serves the whole catalogue, so there is no local
    /// binary to install and "reachable" is the only meaningful health signal.
    /// The catalogue endpoint lists models the ACCOUNT may not be entitled to,
    /// so a model is only trusted once a real probe has answered.
    Nvidia,
    Custom,
}

// ─── Server / scan / agent-access settings requests ───────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetScanPathsRequest {
    pub paths: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentAccessRequest {
    pub agent: AgentType,
    pub full_access: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentConcurrencyRequest {
    pub agent: AgentType,
    /// `None` restores the family default: unlimited for a remote provider,
    /// 1 for Ollama, 5 for a CLI.
    pub concurrency: Option<u32>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetAgentMentionColorRequest {
    pub agent: AgentType,
    /// `None` or an empty string restores the built-in color.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ServerConfigPublic {
    pub host: String,
    pub port: u16,
    pub domain: Option<String>,
    pub max_concurrent_agents: usize,
    pub agent_stall_timeout_min: u32,
    pub agent_global_timeout_min: u32,
    pub local_agent_global_timeout_min: u32,
    pub auth_enabled: bool,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: bool,
    /// Whether discussion composers expose the out-of-context note control.
    pub discussion_notes_enabled: bool,
    /// 0.8.6 phase 4 — default model tier for new disc/QP/WF agent steps.
    /// Mirrored from `ServerConfig.default_model_tier` so the frontend
    /// can pre-fill the tier picker on creation forms without an extra
    /// round-trip. Strict semantic — never retroactive (see backing field
    /// rustdoc).
    pub default_model_tier: ModelTier,
    /// 0.8.6 phase 4 — default summary strategy for new discussions.
    /// `Off` by default in 0.8.6 onwards. UI surfaces an explanation of
    /// when to re-enable (small-context agents without MCP access).
    pub default_summary_strategy: crate::models::SummaryStrategy,
    pub agent_handoffs_enabled: bool,
    pub agent_handoff_paid_limit: u32,
    pub agent_handoff_paid_unlimited: bool,
    pub agent_handoff_blocked_agents: Vec<AgentType>,
    /// Sidebar storage-weight indicator: lets the frontend skip the batch
    /// call entirely when disabled, and grade colours without a round-trip.
    pub discussion_weight: crate::models::DiscussionWeightConfig,
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerConfigRequest {
    pub domain: Option<String>,
    pub max_concurrent_agents: Option<usize>,
    pub agent_stall_timeout_min: Option<u64>,
    pub agent_global_timeout_min: Option<u64>,
    pub local_agent_global_timeout_min: Option<u64>,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: Option<bool>,
    #[serde(default)]
    pub discussion_notes_enabled: Option<bool>,
    /// 0.8.6 phase 4 — `Some(tier)` writes the new default ; `None`
    /// keeps the existing value (standard PATCH semantic across this
    /// struct).
    #[serde(default)]
    pub default_model_tier: Option<ModelTier>,
    /// 0.8.6 phase 4 — `Some(strategy)` writes the new default ;
    /// `None` keeps the existing value.
    #[serde(default)]
    pub default_summary_strategy: Option<crate::models::SummaryStrategy>,
    #[serde(default)]
    pub agent_handoffs_enabled: Option<bool>,
    #[serde(default)]
    pub agent_handoff_paid_limit: Option<u32>,
    #[serde(default)]
    pub agent_handoff_paid_unlimited: Option<bool>,
    #[serde(default)]
    pub agent_handoff_blocked_agents: Option<Vec<AgentType>>,
    /// Whole section at once: the toggle and the pair are persisted together
    /// so a rejected pair can never leave a half-applied state.
    pub discussion_weight: Option<crate::models::DiscussionWeightConfig>,
}
