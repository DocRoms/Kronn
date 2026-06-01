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
    /// UI language (FR/EN/ES) for the React frontend. Persisted here so a
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
    /// Maximum concurrent agent processes (default: 5)
    #[serde(default = "default_max_agents")]
    pub max_concurrent_agents: usize,
    /// Agent stall timeout in minutes — abort if no output for this long (default: 5)
    #[serde(default = "default_agent_stall_timeout")]
    pub agent_stall_timeout_min: u32,
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
    /// 0.9.0 — Continual Learning master toggle. **Default OFF (beta)**: the
    /// feature writes agent-proposed learnings into injected truth files
    /// (`docs/learnings.md` / user-context), so it ships opt-in to avoid a bug
    /// polluting a user's docs. Gates capture (`learning_propose`), the
    /// `kronn:section name="learnings"` doc pointer, and the UI badge/modal.
    /// Validating/rejecting EXISTING pending candidates stays allowed when off
    /// (drain, don't capture). See docs/research/continual-learning-0.9.0-spec.md §0.
    #[serde(default)]
    pub continual_learning_enabled: bool,
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
    /// Re-enable `Auto` (Settings) when running small-context agents
    /// (Ollama 8B / Vibe / older models) that lack MCP access and
    /// can't ask Kronn for older history themselves.
    ///
    /// Strict semantic — only consulted on NEW disc creation. Existing
    /// discs keep their saved value (no retroactive change).
    #[serde(default = "default_summary_strategy_off")]
    pub default_summary_strategy: crate::models::SummaryStrategy,
}

/// Serde default for [`ServerConfig::default_summary_strategy`].
/// Returns `Off` so a missing field in config.toml means "auto-summary
/// disabled" — the new safer default shipped 0.8.6 phase 4.
fn default_summary_strategy_off() -> crate::models::SummaryStrategy {
    crate::models::SummaryStrategy::Off
}

fn default_global_context_mode() -> String { "always".to_string() }
fn default_anti_hallucination_mode() -> String { crate::core::anti_halluc::DEFAULT_MODE_STR.to_string() }
fn default_max_agents() -> usize { 5 }
fn default_agent_stall_timeout() -> u32 { 5 }

/// Default output language. Used by `AppConfig.language` AND by API
/// request types deserialized from the frontend (where the user may
/// omit the field). `pub(crate)` so other model sub-modules can keep
/// the `default = "..."` attribute working after extraction.
pub(crate) fn default_language() -> String { "fr".into() }
pub(crate) fn default_ui_language() -> String { "fr".into() }

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
        self.keys.iter()
            .find(|k| k.provider == provider && k.active)
            .map(|k| k.value.as_str())
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

fn default_scan_depth() -> usize { 4 }

// ─── Agents ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsConfig {
    pub claude_code: AgentConfig,
    pub codex: AgentConfig,
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
    /// Per-agent model tier overrides (Economy/Reasoning model names).
    #[serde(default)]
    pub model_tiers: ModelTiersConfig,
}

impl AgentsConfig {
    /// Get the full_access setting for a given agent type.
    pub fn full_access_for(&self, agent: &AgentType) -> bool {
        match agent {
            AgentType::ClaudeCode => self.claude_code.full_access,
            AgentType::Codex => self.codex.full_access,
            AgentType::GeminiCli => self.gemini_cli.full_access,
            AgentType::Kiro => self.kiro.full_access,
            AgentType::Vibe => self.vibe.full_access,
            AgentType::CopilotCli => self.copilot_cli.full_access,
            AgentType::Ollama => self.ollama.full_access,
            _ => false,
        }
    }

    pub fn any_full_access(&self) -> bool {
        self.claude_code.full_access
            || self.codex.full_access
            || self.gemini_cli.full_access
            || self.kiro.full_access
            || self.vibe.full_access
            || self.copilot_cli.full_access
            || self.ollama.full_access
    }

    /// Returns true if at least one agent is marked as installed.
    pub fn any_installed(&self) -> bool {
        self.claude_code.installed
            || self.codex.installed
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
    pub gemini_cli: ModelTierConfig,
    #[serde(default)]
    pub kiro: ModelTierConfig,
    #[serde(default)]
    pub vibe: ModelTierConfig,
    #[serde(default)]
    pub copilot_cli: ModelTierConfig,
    #[serde(default)]
    pub ollama: ModelTierConfig,
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
    ///
    /// `None` means "no degradation detected, agent is healthy".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_warning: Option<String>,
}

fn default_true() -> bool { true }

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
    Vibe,
    GeminiCli,
    Kiro,
    CopilotCli,
    /// Local LLM via Ollama (0.4.0). CLI: `ollama run <model>`.
    /// Zero tokens, zero cost. MCP via prompt injection (Phase 1).
    Ollama,
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

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ServerConfigPublic {
    pub host: String,
    pub port: u16,
    pub domain: Option<String>,
    pub max_concurrent_agents: usize,
    pub agent_stall_timeout_min: u32,
    pub auth_enabled: bool,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: bool,
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
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerConfigRequest {
    pub domain: Option<String>,
    pub max_concurrent_agents: Option<usize>,
    pub agent_stall_timeout_min: Option<u64>,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
    pub debug_mode: Option<bool>,
    /// 0.8.6 phase 4 — `Some(tier)` writes the new default ; `None`
    /// keeps the existing value (standard PATCH semantic across this
    /// struct).
    #[serde(default)]
    pub default_model_tier: Option<ModelTier>,
    /// 0.8.6 phase 4 — `Some(strategy)` writes the new default ;
    /// `None` keeps the existing value.
    #[serde(default)]
    pub default_summary_strategy: Option<crate::models::SummaryStrategy>,
}
