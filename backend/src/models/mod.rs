use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

/// Deserialize an optional field that distinguishes between absent, null, and present.
/// - Absent key → `None` (outer Option is None → use existing value)
/// - Explicit null → `Some(None)` (set to null)
/// - Present value → `Some(Some(value))` (set to value)
fn deserialize_optional_field<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Option::deserialize(deserializer)?))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Setup & Configuration
// ═══════════════════════════════════════════════════════════════════════════════

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
    /// Debug mode — when true, the tracing subscriber is initialized at
    /// `debug` level instead of `info`, producing significantly more
    /// output on stdout. Lets users diagnose agent detection / project
    /// scan issues themselves without needing to set `RUST_LOG` by hand.
    /// Persisted in config.toml so it survives restarts. Toggleable from
    /// the Settings UI or via `./kronn start --debug` (CLI flag wins for
    /// the duration of that run).
    #[serde(default)]
    pub debug_mode: bool,
}

fn default_global_context_mode() -> String { "always".to_string() }

fn default_max_agents() -> usize { 5 }
fn default_agent_stall_timeout() -> u32 { 5 }

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

// ═══════════════════════════════════════════════════════════════════════════════
// Setup Wizard
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SetupStatus {
    pub is_first_run: bool,
    pub current_step: SetupStep,
    pub agents_detected: Vec<AgentDetection>,
    pub scan_paths_set: bool,
    pub repos_detected: Vec<DetectedRepo>,
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
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentType {
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

// ═══════════════════════════════════════════════════════════════════════════════
// Projects & Repositories
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub repo_url: Option<String>,
    pub token_override: Option<TokenOverride>,
    pub ai_config: AiConfigStatus,
    #[serde(default)]
    pub audit_status: AiAuditStatus,
    #[serde(default)]
    pub ai_todo_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefing_notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenOverride {
    pub provider: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AiConfigStatus {
    pub detected: bool,
    pub configs: Vec<AiConfigType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AiConfigType {
    ClaudeMd,       // CLAUDE.md
    ClauseDir,      // .claude/
    AiDir,          // .ai/
    CursorRules,    // .cursorrules
    ContinueDev,    // .continue/
    McpJson,        // .mcp.json
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DetectedRepo {
    pub path: String,
    pub name: String,
    pub remote_url: Option<String>,
    pub branch: String,
    pub ai_configs: Vec<AiConfigType>,
    pub has_project: bool,
    pub hidden: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
// AI Audit
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AiAuditStatus {
    #[default]
    NoTemplate,
    TemplateInstalled,
    Bootstrapped,
    Audited,
    Validated,
}

/// Live progress of a running audit, exposed via `GET /api/projects/:id/audit-status`.
///
/// Produced by the three SSE streams (`run_audit`, `partial_audit`, `full_audit`)
/// which write into `AppState.audit_tracker.progress` as they advance. The UI
/// polls this endpoint to "resume" the progress bar when the user navigates
/// away and comes back — no need to restart the audit since the server-side
/// process keeps running.
///
/// The struct is deliberately thin: it carries what's needed to paint a
/// progress bar, not the full audit content (that still flows through SSE
/// when the user is actively connected).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditProgress {
    pub project_id: String,
    /// `"installing"` during template install, `"auditing"` during the 10-step
    /// loop, `"validating"` during phase 3 (validation discussion creation),
    /// `"done"` briefly before the tracker clears the entry.
    pub phase: String,
    pub step_index: u32,
    pub total_steps: u32,
    /// `ai/` file currently being produced (e.g. `"repo-map.md"`), or
    /// `"Final review"` for the last step, or `None` between steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    pub started_at: DateTime<Utc>,
    /// `"full"` for the 10-step audit, `"partial"` for drift-triggered
    /// sub-audits, `"full_audit"` for the end-to-end variant. Kept as a
    /// string so future audit kinds don't force a schema migration.
    pub kind: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct LaunchAuditRequest {
    pub agent: AgentType,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BootstrapProjectRequest {
    pub name: String,
    pub description: String,
    pub agent: AgentType,
    #[serde(default)]
    pub mcp_config_ids: Vec<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BootstrapProjectResponse {
    pub project_id: String,
    pub discussion_id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CloneProjectRequest {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    pub agent: AgentType,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CloneProjectResponse {
    pub project_id: String,
    pub discussion_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RemoteRepo {
    pub name: String,
    pub full_name: String,
    pub clone_url: String,
    pub ssh_url: String,
    pub description: Option<String>,
    pub language: Option<String>,
    pub stargazers_count: u32,
    pub updated_at: String,
    pub source: String,  // "github" or "gitlab"
    pub already_cloned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoSource {
    pub id: String,           // MCP config id, or "env:github" / "env:gitlab"
    pub label: String,        // MCP config label, or "GitHub (env)" / "GitLab (env)"
    pub provider: String,     // "github" or "gitlab"
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct DiscoverReposRequest {
    #[serde(default)]
    pub source_ids: Vec<String>,  // empty = use all available sources
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DiscoverReposResponse {
    pub repos: Vec<RemoteRepo>,
    pub sources: Vec<String>,
    pub available_sources: Vec<RepoSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditInfo {
    pub files: Vec<AuditFileInfo>,
    pub todos: Vec<AuditTodo>,
    #[serde(default)]
    pub tech_debt_items: Vec<TechDebtItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TechDebtItem {
    pub id: String,
    pub problem: String,
    pub area: String,
    pub severity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditFileInfo {
    pub path: String,
    pub filled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditTodo {
    pub file: String,
    pub line: u32,
    pub text: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP (Model Context Protocol) — New 3-tier model
// ═══════════════════════════════════════════════════════════════════════════════

/// An MCP server type (e.g. "GitHub", "Atlassian", "Context7").
///
/// A plugin can have MCP capability (via `transport`), API capability (via
/// `api_spec`), or BOTH — e.g. Jira exposes both a `@modelcontextprotocol`
/// server and a REST API, and a hybrid plugin lets the agent pick the
/// right tool. API-only plugins use `McpTransport::ApiOnly` as a sentinel.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub source: McpSource,
    /// When present, the plugin exposes a REST API. Emitted into the agent's
    /// `--append-system-prompt` as a `=== AVAILABLE APIs ===` block so the
    /// agent can call it via curl (vs. MCP-style tools). NULL = MCP only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_spec: Option<ApiSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
    Streamable { url: String },
    /// Plugin has no MCP transport — it's API-only. The sync code MUST skip
    /// these when writing `.mcp.json`; everything relevant lives in
    /// `api_spec` and gets injected into the prompt instead.
    ApiOnly,
}

/// REST API capability for a plugin.
///
/// Stored on `McpServer` to let a plugin expose an HTTP API alongside (or
/// instead of) an MCP transport. The value is serialized into the
/// `mcp_servers.api_spec_json` column (migration 035) and reused by the
/// prompt-injection path that emits `=== AVAILABLE APIs ===` blocks.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiSpec {
    pub base_url: String,
    pub auth: ApiAuthKind,
    /// Short list of the most useful endpoints. Not exhaustive — the UI
    /// surfaces `docs_url` for the full reference. Agents may call
    /// undocumented endpoints if they know the path; this list is
    /// primarily a hint + curl example.
    pub endpoints: Vec<ApiEndpoint>,
    /// URL of the vendor's API reference documentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// Additional config keys the user must provide on top of the credential
    /// (e.g. Chartbeat's `host=example.com`). Stored alongside the secret
    /// in the config's encrypted env, surfaced in the prompt injection so
    /// the agent has the exact curl arguments to use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_keys: Vec<ApiConfigKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApiAuthKind {
    /// API key passed as a query parameter, e.g. `?apikey=...` (Chartbeat).
    ApiKeyQuery { param_name: String, env_key: String },
    /// API key passed as a header, e.g. `X-API-Key: ...`.
    ApiKeyHeader { header_name: String, env_key: String },
    /// Bearer token in `Authorization: Bearer ...`.
    Bearer { env_key: String },
    /// HTTP Basic — `Authorization: Basic <base64(user:password)>`. Used by
    /// Jira Cloud (`email:api_token`), Bitbucket Cloud, and any other
    /// vendor that ships a "user + secret" token pair. Both halves come
    /// from the encrypted config env so Kronn never stores them in
    /// plaintext.
    Basic { user_env: String, password_env: String },
    /// HTTP Basic with the API key as the username and an empty password
    /// — `Authorization: Basic <base64(API_KEY:)>`. The flavor used by
    /// SpeedCurve, Stripe, and a few other API-key-only providers that
    /// chose Basic as the wire format. Reduces the env-key footprint to
    /// 1 (the secret) and avoids forcing operators to type a placeholder
    /// "empty password" in Settings → APIs.
    BasicApiKey { env_key: String },
    /// OAuth2 client-credentials grant — Kronn exchanges `client_id` +
    /// `client_secret` against `token_url` to get a short-lived
    /// `access_token`, caches it until expiry, and injects a fresh
    /// `Authorization: Bearer <token>` into the agent's prompt.
    ///
    /// `extra_headers` lets the spec declare other headers Kronn should
    /// surface to the agent (e.g. Adobe's `x-api-key: <client_id>` +
    /// `x-proxy-global-company-id: <company_id>`). Values can reference
    /// any env key from the config via `{ENV_KEY}` placeholders and are
    /// substituted at injection time.
    OAuth2ClientCredentials {
        token_url: String,
        client_id_env: String,
        client_secret_env: String,
        scope: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_headers: Vec<OAuth2ExtraHeader>,
    },
    /// No auth (public endpoints).
    None,
}

/// Static header rendered alongside the `Authorization: Bearer` from an
/// OAuth2 exchange. Used for providers (Adobe Analytics, some Salesforce
/// endpoints) that require extra identification headers beyond the
/// bearer token. `value_template` supports `{ENV_KEY}` substitution from
/// the config's env map.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OAuth2ExtraHeader {
    pub name: String,
    pub value_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiEndpoint {
    pub path: String,
    /// `"GET"`, `"POST"`, etc. Kept free-form to avoid constraining agents
    /// that want to call a rare verb.
    pub method: String,
    pub description: String,
}

/// A non-secret parameter the plugin instance needs (e.g. host, workspace id).
/// Stored in the same encrypted env blob as the API key, but the UI renders
/// these as plain inputs (no mask), and they're surfaced in the prompt
/// injection alongside the auth so the agent can build full URLs.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiConfigKey {
    pub env_key: String,
    pub label: String,
    pub placeholder: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpSource {
    Registry,
    /// Surfaced from a project's `.mcp.json` (existing path).
    Detected,
    Manual,
    /// Adopted from a host CLI config file (`~/.claude.json`,
    /// `~/.gemini/settings.json`, etc.) via Phase-2 host-discovery import.
    HostImported,
}

/// How a config should be surfaced to the local CLIs (Claude Code, Gemini,
/// Codex, Copilot) when they run *outside* a Kronn-managed project.
///
/// Separated from `is_global` (which is Kronn-internal: "applied across all
/// Kronn projects") because the two concepts answer different questions:
/// `is_global` decides Kronn project visibility; `host_sync` decides
/// whether Kronn writes the entry into `~/.claude.json` & friends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum HostSyncMode {
    /// Kronn-only. Never written to a host CLI config file.
    None,
    /// Written to host config files. Not auto-applied to Kronn projects.
    GlobalOnly,
    /// Written to host config files AND auto-applied to all projects
    /// (preserves the pre-0.5.2 Codex/Copilot "everything global" UX).
    MirrorAll,
}

/// A configured instance of an MCP server — with label, env secrets, etc.
/// Multiple projects can share the same config (deduplication by config_hash).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpConfig {
    pub id: String,
    pub server_id: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_encrypted: String,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub include_general: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
    /// Migration 036 — opt-in outbound sync to host CLI files.
    /// Defaults to `None` for safety; existing rows get `None` from the
    /// SQL migration's `DEFAULT 'None'` clause.
    #[serde(default = "default_host_sync")]
    pub host_sync: HostSyncMode,
}

fn default_host_sync() -> HostSyncMode { HostSyncMode::None }

/// Display-safe version of McpConfig (secrets masked)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpConfigDisplay {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_masked: Vec<McpEnvEntry>,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub include_general: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
    pub project_names: Vec<String>,
    /// True when env_keys exist but decryption fails (secrets need re-entry).
    #[serde(default)]
    pub secrets_broken: bool,
    /// See `McpConfig::host_sync`.
    #[serde(default = "default_host_sync")]
    pub host_sync: HostSyncMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpEnvEntry {
    pub key: String,
    pub masked_value: String,
}

/// Registry entry — an MCP available for installation
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub env_keys: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_help: Option<String>,
    /// Who built this MCP server (e.g. "Anthropic", "Redis Labs", "Fastly").
    pub publisher: String,
    /// True when the MCP is built by the vendor of the service it connects to
    /// (e.g. Fastly MCP by Fastly = official, GitHub MCP by Anthropic = not official by vendor).
    pub official: bool,
    /// Alternative package names that map to this same MCP server.
    /// Used during scan to match detected .mcp.json entries that use a different
    /// runtime (e.g. npm package vs Go binary) to the canonical registry entry.
    /// Example: Fastly registry uses `fastly-mcp` (Go) but users may have `fastly-mcp-server` (npm).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub alt_packages: Vec<String>,
    /// Pre-filled MCP context content (best practices, token-saving tips).
    /// Written to ai/operations/mcp-servers/<slug>.md on first install instead of empty template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub default_context: Option<String>,
    /// API capability declaration when the plugin exposes a REST API
    /// (pure-API like Chartbeat, or hybrid like Jira which has both an MCP
    /// and a REST API). Mirrored onto the corresponding `McpServer.api_spec`
    /// at seed time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_spec: Option<ApiSpec>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick Prompts (reusable prompt templates with variables)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PromptVariable {
    pub name: String,
    pub label: String,
    pub placeholder: String,
    /// Optional human description of what this variable means. Shown in
    /// the batch-workflow UI so the user mapping tracker fields to QP
    /// variables knows what each one is for.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the variable must be filled before the QP can run.
    /// Defaults to `true` for backward compatibility — existing QP
    /// variables are treated as required.
    #[serde(default = "default_variable_required")]
    pub required: bool,
}

fn default_variable_required() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPrompt {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub prompt_template: String,
    pub variables: Vec<PromptVariable>,
    pub agent: AgentType,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    /// Optional human description of what this Quick Prompt does. Shown
    /// in the batch-workflow picker so the user knows which QP fits their
    /// use case. Empty string = legacy QP created before 2026-04-10.
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickPromptRequest {
    pub name: String,
    pub icon: Option<String>,
    pub prompt_template: String,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
    pub agent: Option<AgentType>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub tier: ModelTier,
    #[serde(default)]
    pub description: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Quick APIs (0.6.0) — reusable API call templates with {{variables}}.
// Same pattern as QuickPrompt but the engine is HTTP, not LLM. Field names
// follow `WorkflowStep` ApiCall fields verbatim so the frontend can reuse
// `ApiCallStepCard` (and therefore `ApiCallAiHelper`) as the editor.
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApi {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Optional human description — shown in the BatchApiCall picker.
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,

    // API request shape — same field names as WorkflowStep ApiCall fields.
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_query: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    /// Same shape as `WorkflowStep.api_body`: a JSON `Value` rather than a
    /// raw string. The runtime engine walks the tree and interpolates
    /// string leaves only — no string-level templating that would let a
    /// `{{var}}` containing `","` punch through into JSON injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_body: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_extract: Option<ExtractSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_pagination: Option<PaginationSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_max_retries: Option<u8>,

    /// Variables prompted at run-time (single-call) or whose names become
    /// the keys mapped from each batch item (batch-call).
    pub variables: Vec<PromptVariable>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateQuickApiRequest {
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub description: String,
    pub project_id: Option<String>,
    pub api_plugin_slug: String,
    pub api_config_id: String,
    pub api_endpoint_path: String,
    pub api_method: Option<String>,
    pub api_query: Option<std::collections::HashMap<String, String>>,
    pub api_path_params: Option<std::collections::HashMap<String, String>>,
    pub api_headers: Option<std::collections::HashMap<String, String>>,
    pub api_body: Option<serde_json::Value>,
    pub api_extract: Option<ExtractSpec>,
    pub api_pagination: Option<PaginationSpec>,
    pub api_timeout_ms: Option<u64>,
    pub api_max_retries: Option<u8>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflows (replaces scheduled tasks)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub project_id: Option<String>,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
    pub actions: Vec<WorkflowAction>,
    pub safety: WorkflowSafety,
    pub workspace_config: Option<WorkspaceConfig>,
    pub concurrency_limit: Option<u32>,
    /// Execution limits (timeout, LLM calls cap, loop detection). 0.7.0 —
    /// Phase 1 of the Auto-Dev workflow expansion. `None` = use the soft
    /// backend defaults (120 min wall-clock, 100 LLM calls, 10 revisits
    /// per step) so existing workflows get the safety net automatically.
    /// Explicit `Some(WorkflowGuards { ... })` lets users override per
    /// workflow without touching server config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guards: Option<WorkflowGuards>,
    /// 0.7.0 Phase 3 — declared artifacts the workflow's steps may write.
    /// Map key = artifact name (referenced in steps as `{{artifacts.<name>}}`).
    /// Value = relative path inside the run's workspace where Kronn
    /// persists whatever the agent emits in `---ARTIFACT:<name>---...---END_ARTIFACT---`.
    /// Empty by default (rétro-compat). Reading an undeclared artifact
    /// from a template renders empty string — no hard error so partial
    /// pipelines (artifact only set on round 2+ of a loop) keep flowing.
    #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, ArtifactSpec>")]
    pub artifacts: ::std::collections::HashMap<String, ArtifactSpec>,
    /// 0.7.0 Phase 7 — compensating steps run when the main pipeline ends
    /// in `RunStatus::Failed`. Empty by default (rétro-compat). NOT fired on
    /// Cancelled / StoppedByGuard / Gate-Reject — those are intentional
    /// stops, the operator doesn't want any further automation. Each
    /// rollback step sees the regular template context PLUS
    /// `{{failed_step.name}}` and `{{failed_step.output}}` so the
    /// rollback can react to what specifically broke. If a rollback step
    /// itself fails, subsequent rollback steps are skipped (no recursive
    /// compensation) — the run remains `Failed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_failure: Vec<WorkflowStep>,
    /// 0.7.0 Phase 5 — allowlist of binaries that `StepType::Exec` is
    /// permitted to invoke for this workflow. Empty list = `Exec` steps
    /// are completely disabled (default: safe). Match is exact on the
    /// binary name (no glob, no regex, no path), so `npm` and
    /// `/usr/local/bin/npm` are different — only the bare name passes.
    /// Validate-time error when an Exec step's `exec_command` isn't in
    /// this list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_allowlist: Vec<String>,
    /// 0.6.0 UX pass — variables prompted at manual launch time (mirrors
    /// `QuickPrompt.variables`). When the user clicks "Lancer" on a
    /// workflow with `trigger == Manual` and `!variables.is_empty()`,
    /// the launcher shows a form asking for one value per variable;
    /// the values are merged into the run's `trigger_context` so they
    /// resolve as `{{var_name}}` in step prompts. Empty for trigger-
    /// driven workflows that get their context from the trigger
    /// (issue.* / cron payload). Required variables fail launch when
    /// the value is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<PromptVariable>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Declared artifact in a workflow. Phase-3 minimal model — only
/// path + optional format hint. Path is resolved relative to the run's
/// workspace; absolute paths and `..` traversal are rejected at
/// validate-time (`validate_artifact_specs`).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ArtifactSpec {
    /// Workspace-relative path (e.g. `.kronn/plan.md`).
    pub path: String,
    /// Hint for the UI — `"markdown"`, `"yaml"`, `"json"`, `"text"` —
    /// informational only, the engine doesn't enforce a format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Per-workflow execution limits enforced by the runner. Each field is
/// optional: `None` means "use the runner's soft default". 0 / negative
/// values are rejected at save time (`api::workflows::validate_guards`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct WorkflowGuards {
    /// Wall-clock max duration of the run from `WorkflowRun.started_at`.
    /// Includes time spent in `WaitingApproval` (Phase 2 GATE) UNLESS the
    /// runner is later updated to pause the timer there. Triggers
    /// `RunStatus::StoppedByGuard` + `RunEvent::GuardTriggered { kind: Timeout }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,

    /// Hard cap on the number of LLM-spending steps. `Agent` counts as 1,
    /// `BatchQuickPrompt` counts as N (post-fan-out, after items are
    /// resolved), `ApiCall` and `Notify` count as 0. Prevents a Goto
    /// loop or a misconfigured workflow from burning a budget overnight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_llm_calls: Option<u32>,

    /// Max number of times the runner is allowed to revisit the same
    /// step (via `ConditionAction::Goto`). Per-step counter, not total
    /// iterations — a 100-step linear workflow won't trigger this.
    /// Defaults to 10. Triggers `RunStatus::StoppedByGuard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_detection_max_revisits: Option<usize>,
}

/// Soft backend defaults applied when `Workflow.guards` is `None` or any
/// individual field is `None`. Acts as a kill-switch against runaway runs
/// without forcing every user to configure the limits manually.
pub const DEFAULT_GUARD_TIMEOUT_SECS: u64 = 7200;       // 2 hours
pub const DEFAULT_GUARD_MAX_LLM_CALLS: u32 = 100;
pub const DEFAULT_GUARD_LOOP_MAX_REVISITS: usize = 10;

impl WorkflowGuards {
    /// Resolve the effective limits, falling back to backend defaults.
    /// Always returns concrete values — never `None`.
    pub fn resolved(&self) -> ResolvedGuards {
        ResolvedGuards {
            timeout_seconds: self.timeout_seconds.unwrap_or(DEFAULT_GUARD_TIMEOUT_SECS),
            max_llm_calls: self.max_llm_calls.unwrap_or(DEFAULT_GUARD_MAX_LLM_CALLS),
            loop_detection_max_revisits: self.loop_detection_max_revisits
                .unwrap_or(DEFAULT_GUARD_LOOP_MAX_REVISITS),
        }
    }

    /// Resolve from an `Option<WorkflowGuards>` — `None` (no overrides at
    /// all) yields full defaults.
    pub fn resolve_optional(opt: Option<&WorkflowGuards>) -> ResolvedGuards {
        opt.map(|g| g.resolved()).unwrap_or_else(|| WorkflowGuards::default().resolved())
    }
}

/// All-fields-resolved variant used internally by the runner so the
/// guard-check code never has to deal with `Option`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedGuards {
    pub timeout_seconds: u64,
    pub max_llm_calls: u32,
    pub loop_detection_max_revisits: usize,
}

/// Which guard tripped — surfaced verbatim in the SSE `GuardTriggered`
/// event so the frontend can render the right badge / toast / explainer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum GuardKind {
    Timeout,
    MaxLlmCalls,
    LoopDetection { step_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum WorkflowTrigger {
    Cron {
        schedule: String,
    },
    Tracker {
        source: TrackerSourceConfig,
        query: String,
        labels: Vec<String>,
        interval: String,
    },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum TrackerSourceConfig {
    GitHub {
        owner: String,
        repo: String,
    },
}

/// How a step's output is formatted and extracted.
/// `FreeText` (default): raw text, passed as-is via `{{previous_step.output}}`.
/// `Structured`: engine injects format instructions and extracts a JSON envelope
///   (`{"data": ..., "status": "OK|NO_RESULTS|ERROR", "summary": "..."}`).
///   Downstream steps can use `{{previous_step.data}}` and `{{previous_step.summary}}`.
/// `TypedSchema { schema }` (0.7.0 Phase 2): like Structured, but the
///   `data` field is validated against a JSON-Schema subset provided
///   by the workflow author. The schema is serialised into the prompt
///   so the LLM produces conforming output, and the engine rejects
///   non-conforming responses with a repair prompt. Used by Auto-Dev's
///   `validate_ticket` step (output_schema with status enum + score range).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepOutputFormat {
    #[default]
    FreeText,
    Structured,
    /// 0.7.0 Phase 2 — like `Structured` but the `data` field is
    /// constrained by a JSON-Schema subset (`type`, `properties`,
    /// `required`, `enum`, `items`, `min`/`max`, `minLength`/`maxLength`).
    /// The schema is serialised into the prompt and validated post-extract.
    /// Mismatches trigger a single repair prompt (same pattern as the
    /// vanilla `Structured` envelope flow).
    TypedSchema {
        /// JSON Schema (subset). Stored verbatim; the runner serialises
        /// it into the prompt as-is so the LLM sees the exact shape it
        /// must produce.
        #[ts(type = "any")]
        schema: serde_json::Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowStep {
    pub name: String,
    #[serde(default)]
    pub step_type: StepType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub agent: AgentType,
    pub prompt_template: String,
    pub mode: StepMode,
    #[serde(default)]
    pub output_format: StepOutputFormat,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_config_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_settings: Option<AgentSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_result: Vec<StepConditionRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_after_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directive_ids: Vec<String>,

    // ─── BatchQuickPrompt fields ─────────────────────────────────────────
    // All Option<> so existing Agent/ApiCall steps deserialize unchanged.
    // Only meaningful when `step_type == BatchQuickPrompt`.

    /// Id of the Quick Prompt to fan out. Required for BatchQuickPrompt steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_quick_prompt_id: Option<String>,

    /// Template expression that resolves to the list of items. Each item
    /// becomes one child discussion. Examples:
    /// - `"{{steps.fetch_tickets.data.tickets}}"` — structured JSON array
    /// - `"{{steps.fetch_tickets.output}}"` — raw text (parsed as one id per line)
    ///
    /// Required for BatchQuickPrompt steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_items_from: Option<String>,

    /// If true (default), the linear workflow run waits for all child
    /// discussions to finish before moving to the next step. Uses the existing
    /// `BatchRunFinished` WS broadcast as the wake signal — no polling.
    /// If false, the batch is fired and the linear run advances immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_wait_for_completion: Option<bool>,

    /// Safety cap for the number of items spawned by this step. Falls back to
    /// the global 50-item cap enforced by `create_batch_run` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_max_items: Option<u32>,

    /// Workspace mode for each batch child discussion: `"Direct"` (default)
    /// or `"Isolated"` for per-disc git worktrees. Isolated is required when
    /// the agents will write code in parallel — otherwise they clobber each
    /// other in the main working tree. Requires the workflow to have a
    /// project_id, otherwise the step fails early.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_workspace_mode: Option<String>,

    /// Chain additional Quick Prompts after the initial one inside each
    /// child discussion. Each QP is auto-sent as a User message once the
    /// previous agent response completes, and the agent is re-fired.
    /// The batch progress counter only increments after the ENTIRE chain
    /// (initial QP + all chained QPs) finishes for a given discussion.
    /// Example: `["qp-review", "qp-summary"]` after the primary `batch_quick_prompt_id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub batch_chain_prompt_ids: Vec<String>,

    /// Concurrent fan-out cap for `StepType::BatchApiCall` (HTTP path only).
    /// `None` falls back to a conservative default (5). HTTP can scale much
    /// higher than agent runs (no LLM, just network) but providers rate-limit
    /// — Jira/GitHub typically OK up to 10-20 in parallel, beyond that you
    /// risk 429s. Distinct from BatchQuickPrompt (which goes through the
    /// global agent_semaphore).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_concurrent_limit: Option<u32>,

    /// 0.6.0 — when set on a `BatchApiCall` step, the executor loads the
    /// referenced `QuickApi` from the DB at run-time and uses its API
    /// config (plugin, endpoint, method, body, etc.) instead of the
    /// step's own inline `api_*` fields. Mirror of `batch_quick_prompt_id`.
    /// `None` keeps inline-config behaviour. 0.7+ — étendu à `StepType::ApiCall`
    /// (single, non-batch) avec la même sémantique per-field override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_api_id: Option<String>,

    /// 0.7+ — référence vers un `QuickPrompt` saved. Quand set sur un step
    /// `Agent`, le runner charge le QP au run-time et utilise son
    /// `prompt_template`, son `agent`, son `tier`, et ses `skill_ids` ; les
    /// fields renseignés sur le step écrasent ceux du QP (per-field override).
    /// Permet de définir un prompt canonique côté Quick Prompts et de le
    /// réutiliser dans N workflows. Pas de variables au niveau step :
    /// les `{{var}}` du QP sont résolus avec le `TemplateContext` du
    /// workflow (launch variables / state / steps.X / etc.). Mirror du
    /// pattern `quick_api_id` pour les ApiCall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_prompt_id: Option<String>,

    // ─── Notify fields ───────────────────────────────────────────────────
    // Only meaningful when `step_type == Notify`. Webhook-based workflow
    // finalizer: posts to an external URL with a rendered body. Zero agent
    // tokens consumed — direct HTTP from Rust.

    /// Webhook configuration for `StepType::Notify`. URL and body support
    /// the same `{{steps.X.output}}` / `{{steps.X.data}}` templates as
    /// agent prompts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_config: Option<NotifyConfig>,

    // ─── ApiCall fields (désagentification — 0.5.2) ──────────────────────
    // Only meaningful when `step_type == ApiCall`. Calls a Kronn-configured
    // API plugin directly from the Rust engine — zero agent tokens. Params
    // support the same `{{steps.X.data}}` templates as agent prompts.
    // See `ai/operations/deagent-apicall.md` for the full contract.

    /// Registry slug of the plugin to invoke (e.g. `"chartbeat"`, `"jira"`).
    /// The slug resolves to an `ApiSpec` in the plugin registry; the request
    /// base URL comes from that spec and is NEVER templated from the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_plugin_slug: Option<String>,

    /// `McpConfig.id` of the specific credential set to use. The plugin can
    /// be configured multiple times per project (e.g. two Jira instances);
    /// this picks one. Decrypted env lives in the DB row and is loaded at
    /// step execution via `collect_active_api_plugins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_config_id: Option<String>,

    /// Endpoint path as declared in `ApiSpec.endpoints[].path` — prefix-
    /// matched against the allowlist in the executor so a step can't reach
    /// arbitrary paths under the plugin's `base_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_endpoint_path: Option<String>,

    /// HTTP method override. Defaults to the method of the endpoint in the
    /// plugin registry. Uppercase: `GET | POST | PUT | PATCH | DELETE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_method: Option<String>,

    /// Path-segment parameters (e.g. `/repos/{owner}/{repo}` → `{owner}` and
    /// `{repo}`). The executor scans `api_endpoint_path` for `{key}` tokens
    /// and substitutes each match with the value from this map at request
    /// time. Values support `{{steps.X.data}}` templates so a previous
    /// fetch can drive the segment dynamically. Tokens with no entry stay
    /// literal — the request will fail because `/repos/{owner}/...` is not
    /// a real GitHub path. This way the spec-declared template stays in
    /// `api_endpoint_path` (round-trip safe across re-edits) while the
    /// concrete values live separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_path_params: Option<std::collections::HashMap<String, String>>,

    /// Query-string parameters. Values support `{{steps.X.data}}` templates.
    /// Rendered values are percent-encoded AFTER template expansion to
    /// prevent injection (`&` / `=` in a templated value would corrupt the
    /// query otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_query: Option<std::collections::HashMap<String, String>>,

    /// Extra headers (auth headers come from the plugin spec, not here).
    /// String values templatable; keys are literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_headers: Option<std::collections::HashMap<String, String>>,

    /// JSON body for POST/PUT/PATCH. Rendered by walking the `Value` tree
    /// and interpolating string leaves only — no string-level interpolation,
    /// which would allow JSON injection via templated content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_body: Option<serde_json::Value>,

    /// How to extract a value from the JSON response. The extracted `data`
    /// is what downstream steps read via `{{steps.X.data}}`; batch QP steps
    /// expect an array and fail-fast if it's a scalar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_extract: Option<ExtractSpec>,

    /// Pagination strategy. `Auto` (default-ish) inspects the response for
    /// `nextPageToken` / `startAt`+`total` / `page` and walks accordingly,
    /// concatenating arrays. Hard-capped at 50 pages to prevent runaway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_pagination: Option<PaginationSpec>,

    /// Per-request timeout in milliseconds. Defaults to 30 000 ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_timeout_ms: Option<u64>,

    /// Max retries on 5xx / 429 with exponential backoff. Defaults to 2.
    /// Idempotent GETs retry freely; endpoints flagged `side_effect: true`
    /// in the plugin spec skip retry entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_max_retries: Option<u8>,

    /// Context variable name under which the extracted data is stored.
    /// Downstream steps reference it as `{{steps.<output_var>.data}}`.
    /// Defaults to the step's `name` field when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_output_var: Option<String>,

    // ─── Gate fields (0.7.0 Phase 4 — human-in-the-loop) ─────────────
    // Only meaningful when `step_type == Gate`. The runner stops the
    // run with `RunStatus::WaitingApproval`; a human decides via the
    // dashboard. Templates resolve at gate-execution time so the
    // operator sees the actual values, not the literal `{{X}}`.

    /// Markdown message shown to the operator on the run-detail page.
    /// Templates supported. Empty string falls back to a default
    /// "Décision humaine requise" placeholder in the UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_message: Option<String>,

    /// Step name to jump to when the operator picks "Request Changes".
    /// `None` → falls back to the previous step (one step back), which
    /// matches the Auto-Dev `pause_pre_merge` → `goto: implement` pattern.
    /// Set explicitly to a step name for non-default targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_request_changes_target: Option<String>,

    /// 0.7.0 P1-1 — optional webhook URL to POST when the run enters
    /// `WaitingApproval` on this gate. Best-effort fire-and-forget;
    /// failures are logged but never block the run. Templated, so
    /// users can drop `{{state.slack_url}}` etc. Body :
    /// `{run_id, workflow_id, workflow_name, step_name, message}`.
    /// The "ping ops when a Gate fires" use case Cyndie + Antony
    /// flagged as blocker for team-wide deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_notify_url: Option<String>,

    // ─── Exec fields (0.7.0 Phase 5 — direct shell, no LLM) ──────────────
    // Only meaningful when `step_type == Exec`. Defence in depth:
    //   1. `command` is the binary name verbatim — match-tested against
    //      `Workflow.exec_allowlist` at save time (rejected if absent).
    //   2. NEVER passed through a shell (`sh -c`, `bash -c`) — spawned
    //      directly via `crate::core::cmd::async_cmd` with args as
    //      separate argv elements. So pipes, redirections, glob
    //      expansion DO NOT apply.
    //   3. Args ARE templated (`{{steps.X.summary}}` etc.) but the
    //      rendered value is passed as a single literal argument — even
    //      if it contains `; rm -rf /`, the OS receives one argv string,
    //      not a shell command.
    //   4. `command` itself is NOT templated — locked at save time.

    /// Binary to execute. Must match an entry in `Workflow.exec_allowlist`
    /// exactly (no glob, no regex). NOT templated — locked at save time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_command: Option<String>,

    /// Arguments passed verbatim to the binary. Each entry is one argv
    /// element. Templates `{{steps.X}}` are rendered, but the result
    /// becomes a literal argument — no shell metachar interpretation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exec_args: Vec<String>,

    /// Per-step timeout in seconds. Defaults to 300s (5 min) if unset.
    /// Hard-capped at 1800s (30 min) at validate time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_timeout_secs: Option<u32>,

    // ─── JsonData fields (0.7+ — déterministe data source) ───────────────
    // Only meaningful when `step_type == JsonData`. Zéro token, zéro
    // réseau. Le runner sérialise `json_data_payload` dans une envelope
    // Structured et la passe au step suivant. Cas d'usage : alimenter un
    // BatchQuickPrompt sans API derrière. Voir json_data_step.rs.

    /// Payload JSON émis par le step. Validé au save (parse JSON valide,
    /// taille raisonnable). Aucun templating au runtime — la valeur est
    /// retournée telle quelle, ce qui permet à un downstream batch de la
    /// consommer via `{{steps.<name>.data}}` exactement comme une réponse
    /// API. Si tu as besoin de `{{var}}` dans le payload, mets ça dans
    /// un Agent step ou un ApiCall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_data_payload: Option<serde_json::Value>,
}

/// Extraction specification for an `ApiCall` step's JSON response.
/// Implements RFC 9535 JSONPath via the `serde_json_path` crate.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
pub struct ExtractSpec {
    /// JSONPath expression, e.g. `$.issues[*].key` or `$.data.viewer.zones[0].zoneTag`.
    /// Evaluated against the full response (after pagination concat if enabled).
    pub path: String,

    /// Default value when the path resolves to nothing. Keeps workflows
    /// alive on empty results. Omit = `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<serde_json::Value>,

    /// If true and the extraction returns `null` / empty array, the step
    /// reports `status: NO_RESULTS` so `on_result` conditions (Skip, Stop,
    /// Goto) can fire. Default false → status OK even on empty data.
    #[serde(default)]
    pub fail_on_empty: bool,
}

/// Pagination strategy for an `ApiCall` step. `Auto` covers the three most
/// common REST patterns; explicit variants let advanced users hardcode the
/// cursor/offset paths for non-standard APIs (Cloudflare GraphQL for ex.).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(tag = "type")]
pub enum PaginationSpec {
    /// No pagination — issue the request once and return the response.
    None,

    /// Auto-detect: inspects the response for `nextPageToken` (cursor),
    /// `startAt`+`total`+`maxResults` (offset), or `page`+`has_more` (page).
    /// Walks until exhausted or `max_pages` reached.
    Auto {
        /// Safety cap. Defaults to 50 when unset.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Classic offset pagination: `GET …?start_param=0&limit_param=100`,
    /// increments `start` by `limit` until `len(items) < limit` or
    /// `total_path` reports all consumed.
    Offset {
        start_param: String,
        limit_param: String,
        limit: u32,
        /// JSONPath to the total count in the response, e.g. `$.total`.
        total_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Cursor-based: response exposes a next-cursor path that feeds back
    /// into `cursor_param` on the next call. Terminates when the path
    /// resolves to null/absent.
    Cursor {
        cursor_param: String,
        /// JSONPath to the next cursor value, e.g. `$.pageInfo.endCursor`.
        next_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },

    /// Page number: increment `page_param` from 1, stop when `has_more_path`
    /// is false or there are no more results.
    Page {
        page_param: String,
        page_size_param: String,
        page_size: u32,
        /// JSONPath to a boolean / truthy "has more" indicator.
        has_more_path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_pages: Option<u32>,
    },
}

/// Configuration for a `StepType::Notify` webhook step. Rendered at run-time
/// (URL + body support template expressions like `{{previous_step.summary}}`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NotifyConfig {
    /// Target URL. Supports template variables.
    pub url: String,
    /// HTTP method — "POST" (default), "PUT", "GET". Only these three are
    /// accepted; anything else fails at execution time.
    #[serde(default = "default_notify_method")]
    pub method: String,
    /// Custom headers. Case-insensitive on the wire — we send them as given.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: std::collections::HashMap<String, String>,
    /// Request body. Templated. Sent as-is — set `Content-Type: application/json`
    /// in `headers` if the body is JSON. Ignored for GET.
    #[serde(default)]
    pub body_template: String,
}

fn default_notify_method() -> String { "POST".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepMode {
    Normal,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepType {
    #[default]
    Agent,
    ApiCall,
    /// Fan out a Quick Prompt over a list of items (rendered from a previous
    /// step's output) — spawns N child discussions via the shared `create_batch_run`
    /// helper and optionally waits for all of them to finish before moving on.
    /// Phase 2 batch workflows (2026-04-10).
    BatchQuickPrompt,
    /// Direct webhook/HTTP call — zero agent tokens. Used as a finalizer
    /// (send completion notification, trigger downstream pipeline) or as
    /// a mechanical data step (create GitHub issue, post to Slack) without
    /// spawning an LLM. Shipped 0.3.5.
    Notify,
    /// 0.7.0 Phase 4 — human-in-the-loop pause. The run halts at this
    /// step with `RunStatus::WaitingApproval`; a human operator decides
    /// from the UI (`POST /api/workflows/runs/:id/decide`) whether the
    /// workflow continues, jumps to another step (request_changes), or
    /// stops. Zero tokens — no LLM is spawned for the gate itself.
    Gate,
    /// 0.7.0 Phase 5 — direct shell execution. Zero tokens, runs a
    /// pre-allowlisted binary in the workflow's workspace. Defence in
    /// depth: binary must be in `Workflow.exec_allowlist`, NEVER goes
    /// through `sh -c`, args are templated but passed as literal argv
    /// (no shell interpretation), workdir locked to the workspace,
    /// timeout-bounded. Typical use: `cargo test`, `npm run build`,
    /// `make deploy`.
    Exec,
    /// 0.6.0 — fan out an API call over a list of items, in parallel,
    /// **with zero LLM tokens**. The mechanical counterpart of
    /// `BatchQuickPrompt`: same fan-out semantics, but each child fires
    /// a templated HTTP request via the configured plugin instead of
    /// spawning an agent. Used by Feature Planner to bulk-create Jira
    /// tickets (one POST /issue per planned sub-task) without paying
    /// the per-ticket agent loop. Concurrency capped by
    /// `batch_concurrent_limit` (default 5) to avoid hammering the
    /// upstream API.
    BatchApiCall,
    /// 0.7+ — déterministe data source : émet un payload JSON littéral
    /// stocké dans le step (`json_data_payload`). Zéro token, zéro réseau.
    /// Cas d'usage : workflow batch sur une liste figée (ex: 10 hosts
    /// hardcodés alimentent un BatchQuickPrompt sans avoir à monter une
    /// API). Aussi utile comme fixture de dev — on construit le pipeline
    /// sur du JsonData puis on remplace par un `ApiCall` quand la vraie
    /// source est prête. Output toujours `Structured` : envelope
    /// `{data: payload, status: "OK", summary: "JSON data (N items)"}`.
    JsonData,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentSettings {
    /// Explicit model override (expert mode). Takes priority over tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Abstract tier selection. Resolved to a concrete --model flag per agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StepConditionRule {
    pub contains: String,
    pub action: ConditionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum ConditionAction {
    Stop,
    Skip,
    /// Jump back (or forward) to `step_name`. Optional `max_iterations`
    /// scopes the loop: after the same Goto fires that many times,
    /// the runner falls through instead of jumping (run continues
    /// past the loop). Without `max_iterations` the workflow-level
    /// `loop_detection_max_revisits` guard remains the only safety
    /// net — fine for short loops, but per-loop scoping is cleaner
    /// when you have several independent loops in the same workflow.
    /// 0.7.0 Phase 6.
    Goto {
        step_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_iterations: Option<u32>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RetryConfig {
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub backoff: String,
}

fn default_backoff() -> String {
    "exponential".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum WorkflowAction {
    CreatePr {
        title_template: String,
        body_template: String,
        branch_template: String,
    },
    CommentIssue {
        body_template: String,
    },
    UpdateTrackerStatus {
        status: String,
    },
    CreateIssue {
        title_template: String,
        body_template: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSafety {
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_files: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lines: Option<u32>,
    #[serde(default)]
    pub require_approval: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub hooks: WorkspaceHooks,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceHooks {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_create: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_remove: Option<String>,
}

// ─── Workflow Runs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub status: RunStatus,
    #[ts(type = "any")]
    pub trigger_context: Option<serde_json::Value>,
    pub step_results: Vec<StepResult>,
    pub tokens_used: u64,
    pub workspace_path: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Linear workflow run vs batch fan-out. Default "linear" for backward
    /// compatibility with existing runs created before Phase 1b.
    #[serde(default = "default_run_type")]
    pub run_type: String,
    /// For batch runs: target number of child discussions. 0 for linear runs.
    #[serde(default)]
    pub batch_total: u32,
    /// For batch runs: number of successfully-completed child discussions.
    #[serde(default)]
    pub batch_completed: u32,
    /// For batch runs: number of child discussions that ended with an error.
    #[serde(default)]
    pub batch_failed: u32,
    /// For batch runs: display name shown in the sidebar group header.
    /// Example: "Cadrage to-Frame — 10 avr 14:00".
    #[serde(default)]
    pub batch_name: Option<String>,
    /// Link a child batch run back to the linear workflow run that spawned it
    /// via a `BatchQuickPrompt` step. `None` for top-level runs (both linear
    /// runs and manual batch runs triggered from the UI).
    #[serde(default)]
    pub parent_run_id: Option<String>,
    /// 0.7.0 Phase 6 — durable state map carried across iterations and
    /// resume cycles (Gate, restart). Agents write entries by emitting
    /// `---STATE:<key>=<value>---` blocks in their output (parsed
    /// alongside artifacts); steps read them via `{{state.<key>}}`.
    /// Used for retry counters, accumulated verdicts, and any other
    /// cross-iteration memory that doesn't belong in step outputs.
    /// Empty by default. Persisted as a JSON object on the run row.
    #[serde(default, skip_serializing_if = "::std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string>")]
    pub state: ::std::collections::HashMap<String, String>,
}

fn default_run_type() -> String { "linear".to_string() }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    WaitingApproval,
    /// 0.7.0 — terminal state distinct from `Failed`: the run hit a
    /// `WorkflowGuards` limit (timeout, max LLM calls, loop detection).
    /// UX surfaces this with a shield icon (orange, not red) so users
    /// can tell a self-protected stop from a real failure.
    StoppedByGuard,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StepResult {
    pub step_name: String,
    pub status: RunStatus,
    pub output: String,
    pub tokens_used: u64,
    pub duration_ms: u64,
    /// What happened after this step: null = continued normally, or the condition action triggered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition_result: Option<String>,
    /// For `output_format: Structured` steps only — did the agent actually
    /// produce the `---STEP_OUTPUT---` envelope (possibly after repair)?
    /// `Some(true)`  = envelope found, `.data/.summary/.status` populated.
    /// `Some(false)` = Structured requested but extraction failed even after
    ///                 repair, downstream `{{steps.X.data}}` won't resolve.
    /// `None`        = FreeText step, the concept does not apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope_detected: Option<bool>,
    /// Snapshot of the step's `step_type.type` at execution time
    /// (`"Agent" | "ApiCall" | "Notify" | "BatchQuickPrompt" | "Custom"`).
    /// Frozen on the run row so editing the workflow afterwards (changing
    /// the step type, swapping the agent, retargeting the API plugin)
    /// doesn't corrupt the historical record. `None` is tolerated for
    /// rows written before this field existed — the frontend falls back
    /// to "(legacy)" rather than crashing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_kind: Option<String>,
    /// Snapshot of `step.agent` for Agent / Custom steps. `None` for
    /// non-agent steps (ApiCall, Notify, Batch). Lets the run-detail UI
    /// say "Codex was used here" even after the workflow was edited to
    /// run with a different agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_agent: Option<AgentType>,
    /// Snapshot of the API plugin slug for ApiCall steps (`mcp-github`,
    /// `api-chartbeat`, …). `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_api_plugin_slug: Option<String>,
    /// Snapshot of the resolved endpoint path for ApiCall steps. Stored
    /// AFTER path-param substitution so reviewers see the actual URL
    /// path that was hit (`/repos/anthropics/anthropic-cookbook/issues`
    /// rather than the template `/repos/{owner}/{repo}/issues`). `None`
    /// for non-API steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_api_endpoint_path: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Skills (WHAT — domain expertise, multi-select)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SkillCategory {
    Language,
    Domain,
    Business,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: SkillCategory,
    pub content: String,
    pub is_builtin: bool,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
    /// agentskills.io: SPDX license identifier or reference to bundled LICENSE file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// agentskills.io: space-delimited list of pre-approved tools (e.g. "Bash Read Grep").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<String>,
    /// Optional auto-activation trigger regexes keyed by locale. When
    /// the user types a message matching one of these patterns, the
    /// frontend auto-adds this skill to the current discussion. The
    /// `common` entry always applies; the locale-specific entries
    /// apply when the discussion's language matches. See the YAML
    /// frontmatter convention in `backend/src/skills/kronn-docs.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_triggers: Option<AutoTriggers>,
}

/// Auto-trigger regex buckets declared in a skill's frontmatter YAML.
///
/// ```yaml
/// auto_triggers:
///   common:
///     - "\\b(pdf|docx?|xlsx?)\\b"
///   fr:
///     - "génér.+(fichier|rapport)"
///   en:
///     - "generate.+(file|report)"
/// ```
///
/// The frontend combines `common` + the entry matching the discussion
/// language (or `en` as fallback) into a single regex list, and tests
/// every pattern against the pending message.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoTriggers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub common: Vec<String>,
    /// Per-locale patterns keyed by IETF language tag (`fr`, `en`, `es`,
    /// ...). Additional locales can be added without a code change.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    #[ts(type = "Record<string, string[]>")]
    pub locales: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateSkillRequest {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: SkillCategory,
    pub content: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Agent Profiles (WHO — persona, single-select)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ProfileCategory {
    Technical,
    Business,
    Meta,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub persona_name: String,
    pub role: String,
    pub avatar: String,
    pub color: String,
    pub category: ProfileCategory,
    pub persona_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_engine: Option<String>,
    pub is_builtin: bool,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(default)]
    pub persona_name: String,
    pub role: String,
    pub avatar: String,
    pub color: String,
    pub category: ProfileCategory,
    pub persona_prompt: String,
    #[serde(default)]
    pub default_engine: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Directives (HOW — output behavior, multi-select)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DirectiveCategory {
    Output,
    Language,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Directive {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: DirectiveCategory,
    pub content: String,
    pub is_builtin: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// Estimated token cost when injected into an agent prompt (~4 chars = 1 token).
    pub token_estimate: u32,
    /// Optional URL to the source project — set on directives that adapt
    /// third-party prompts (e.g. Caveman → github.com/JuliusBrussee/caveman).
    /// Surfaces as a small "↗ Source" link in the settings card. MIT-licensed
    /// adaptations should include this for attribution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateDirectiveRequest {
    pub name: String,
    pub description: String,
    pub icon: String,
    pub category: DirectiveCategory,
    pub content: String,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// AI Documentation Files (read-only viewer)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiFileNode {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<AiFileNode>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiFileContent {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiSearchResult {
    pub path: String,
    pub match_count: u32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Stats & Analytics
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsageSummary {
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub discussion_tokens: u64,
    pub workflow_tokens: u64,
    pub by_provider: Vec<ProviderUsage>,
    pub by_project: Vec<ProjectUsage>,
    pub top_discussions: Vec<UsageEntry>,
    pub top_workflows: Vec<UsageEntry>,
    pub daily_history: Vec<DailyUsage>,
}

/// A ranked usage entry (for top N lists)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageEntry {
    pub id: String,
    pub name: String,
    pub tokens_used: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderUsage {
    pub provider: String,
    pub tokens_used: u64,
    pub tokens_limit: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectUsage {
    pub project_id: String,
    pub project_name: String,
    pub tokens_used: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentUsageSummary {
    pub agent_type: String,
    pub total_tokens: u64,
    pub message_count: u32,
    pub by_project: Vec<AgentProjectUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentProjectUsage {
    pub project_id: String,
    pub project_name: String,
    pub tokens_used: u64,
    pub message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DailyUsage {
    pub date: String,
    pub tokens: u64,
    pub cost_usd: f64,
    pub anthropic: u64,
    pub openai: u64,
    pub google: u64,
    pub mistral: u64,
    pub amazon: u64,
    pub github: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Contacts (multi-user)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Contact {
    pub id: String,
    pub pseudo: String,
    pub avatar_email: Option<String>,
    pub kronn_url: String,
    pub invite_code: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct AddContactRequest {
    pub invite_code: String,
}

/// Result of adding a contact, with optional diagnostic hint for unreachable peers.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AddContactResult {
    pub contact: Contact,
    /// Human-readable hint explaining why the contact is pending (network mismatch, etc.)
    pub warning: Option<String>,
}

/// Network info for multi-user connectivity.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct NetworkInfo {
    /// Detected Tailscale IPv4 address (100.x.x.x), if available.
    pub tailscale_ip: Option<String>,
    /// The host used in invite codes (domain > tailscale > host).
    pub advertised_host: String,
    /// Backend port.
    pub port: u16,
    /// Configured domain, if any.
    pub domain: Option<String>,
    /// All detected network IPs (tailscale, vpn, lan).
    pub detected_ips: Vec<crate::core::tailscale::DetectedIp>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// WebSocket Protocol
// ═══════════════════════════════════════════════════════════════════════════════

/// Real-time message exchanged between Kronn instances via WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    /// Presence announcement: a peer is online or offline.
    Presence {
        from_pseudo: String,
        from_invite_code: String,
        online: bool,
    },
    /// Heartbeat ping (sent by client).
    Ping { timestamp: i64 },
    /// Heartbeat pong (reply to ping).
    Pong { timestamp: i64 },
    /// Chat message in a shared discussion.
    ChatMessage {
        shared_discussion_id: String,
        message_id: String,
        from_pseudo: String,
        from_avatar_email: Option<String>,
        from_invite_code: String,
        content: String,
        timestamp: i64,
    },
    /// Invitation to join a shared discussion.
    DiscussionInvite {
        shared_discussion_id: String,
        title: String,
        from_pseudo: String,
        from_invite_code: String,
    },
    /// A batch WorkflowRun finished (all child discussions are done).
    /// Sent by the backend to the frontend so the sidebar badge and any
    /// open batch monitors update live.
    BatchRunFinished {
        run_id: String,
        /// Id of the child discussion whose completion triggered the final tick.
        /// The frontend uses it to clear its per-disc `sendingMap` spinner, since
        /// batch children are fire-and-forget (no SSE stream consumer on the client
        /// to drive the usual cleanup path).
        discussion_id: String,
        batch_name: Option<String>,
        batch_total: u32,
        batch_completed: u32,
        batch_failed: u32,
    },
    /// Progress update for a running batch (child disc just finished).
    /// Fires on every child completion so the sidebar pill can tick live.
    BatchRunProgress {
        run_id: String,
        /// Id of the child discussion that just completed — frontend uses it to
        /// clear the per-disc sendingMap indicator.
        discussion_id: String,
        batch_total: u32,
        batch_completed: u32,
        batch_failed: u32,
    },
    /// Broadcast once at backend boot when `recover_partial_responses`
    /// resurrected in-flight agent responses that were cut short by a
    /// restart. Each id in the list got a new Agent message with an
    /// "interrupted" footer — the frontend refetches those discs + toasts
    /// the user so they don't resend their prompt on top of a silently
    /// recovered conversation.
    PartialResponseRecovered {
        discussion_ids: Vec<String>,
    },
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Discussion {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentType,
    pub language: String,
    pub participants: Vec<AgentType>,
    pub messages: Vec<DiscussionMessage>,
    #[serde(default)]
    pub message_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profile_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub archived: bool,
    /// User-pinned / favorite discussion — appears in a dedicated "Favorites"
    /// section at the top of the sidebar regardless of project grouping.
    #[serde(default)]
    pub pinned: bool,
    #[serde(default = "default_workspace_mode")]
    pub workspace_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// Model capability tier for this discussion.
    #[serde(default)]
    pub tier: ModelTier,
    /// Pin the first message (protocol prompt) — always include it in agent prompts, never summarize it.
    /// Used for validation, bootstrap, and briefing discussions.
    #[serde(default)]
    pub pin_first_message: bool,
    /// Cached summary of older messages (eco-design: avoids re-sending full history).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_cache: Option<String>,
    /// Index of the last message included in summary_cache (0-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_up_to_msg_idx: Option<u32>,
    /// Shared discussion UUID (None = local-only, Some = replicated with peers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_id: Option<String>,
    /// Contact IDs this discussion is shared with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
    /// ID of the batch WorkflowRun that spawned this discussion, if any.
    /// Used for sidebar grouping under the project ("Cadrage to-Frame — 10 avr").
    /// Null for manual discussions created outside of a batch workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_run_id: Option<String>,
    /// Test mode — branch the main repo was on before the user entered test
    /// mode. `Some` means the user is actively testing this discussion's
    /// branch in their main repo; `None` means normal worktree operation.
    /// Used by `test-mode/exit` to checkout back to the user's prior state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_mode_restore_branch: Option<String>,
    /// Test mode — if the main repo was dirty at enter time and the user opted
    /// in to auto-stash, this holds the stash message (e.g.
    /// `kronn:auto-<disc_id>`) so `exit` can pop the exact stash.
    /// `None` when the main repo was clean or the user declined the stash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_mode_stash_ref: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn default_workspace_mode() -> String { "Direct".into() }

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionMessage {
    pub id: String,
    pub role: MessageRole,
    pub content: String,
    pub agent_type: Option<AgentType>,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub auth_mode: Option<String>,
    /// Which model tier was used for this message (economy/default/reasoning).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_tier: Option<String>,
    /// Cost in USD (real from Claude Code, estimated for other providers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Author identity (for multi-user / display)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_pseudo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

// ═══════════════════════════════════════════════════════════════════════════════
// API Request/Response types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetScanPathsRequest {
    pub paths: Vec<String>,
}

// ─── Workflow API requests ────────────────────────────────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub project_id: Option<String>,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub actions: Vec<WorkflowAction>,
    #[serde(default)]
    pub safety: Option<WorkflowSafety>,
    #[serde(default)]
    pub workspace_config: Option<WorkspaceConfig>,
    #[serde(default)]
    pub concurrency_limit: Option<u32>,
    #[serde(default)]
    pub guards: Option<WorkflowGuards>,
    #[serde(default)]
    #[ts(type = "Record<string, ArtifactSpec>")]
    pub artifacts: ::std::collections::HashMap<String, ArtifactSpec>,
    #[serde(default)]
    pub on_failure: Vec<WorkflowStep>,
    #[serde(default)]
    pub exec_allowlist: Vec<String>,
    #[serde(default)]
    pub variables: Vec<PromptVariable>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    pub project_id: Option<Option<String>>,
    pub trigger: Option<WorkflowTrigger>,
    pub steps: Option<Vec<WorkflowStep>>,
    pub actions: Option<Vec<WorkflowAction>>,
    pub safety: Option<WorkflowSafety>,
    pub workspace_config: Option<WorkspaceConfig>,
    pub concurrency_limit: Option<u32>,
    pub guards: Option<WorkflowGuards>,
    /// Replace the artifact map entirely when present. To clear all
    /// declarations, send `Some({})`. Omit the field to leave existing
    /// declarations untouched.
    #[serde(default)]
    #[ts(type = "Record<string, ArtifactSpec> | null")]
    pub artifacts: Option<::std::collections::HashMap<String, ArtifactSpec>>,
    /// Replace the rollback chain entirely when present. To clear it,
    /// send `Some([])`. Omit to leave the existing chain untouched.
    #[serde(default)]
    pub on_failure: Option<Vec<WorkflowStep>>,
    /// Replace the Exec allowlist entirely when present. Send
    /// `Some([])` to disable Exec steps; omit to leave it untouched.
    #[serde(default)]
    pub exec_allowlist: Option<Vec<String>>,
    /// Replace launch-time variables entirely when present. `Some([])`
    /// to clear, omit to keep existing.
    #[serde(default)]
    pub variables: Option<Vec<PromptVariable>>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSummary {
    pub id: String,
    pub name: String,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub trigger_type: String,
    pub step_count: u32,
    pub enabled: bool,
    pub last_run: Option<WorkflowRunSummary>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowRunSummary {
    pub id: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub tokens_used: u64,
}

/// Compact summary of a batch workflow run, with its parent linear run
/// resolved to a human-friendly (workflow name + run sequence number) label.
/// Consumed by the discussion sidebar to render a clickable pastille on each
/// batch group ("↗ run #3 de Recap hebdo") so users can trace a batch back
/// to the workflow that spawned it.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchRunSummary {
    pub run_id: String,
    pub batch_name: Option<String>,
    pub batch_total: u32,
    pub status: RunStatus,
    /// Id + name of the Quick Prompt that this batch fans out. Resolved from
    /// the batch run's virtual `qp:<id>` workflow_id prefix. Used by the
    /// sidebar as the batch folder label (instead of the first child disc's
    /// title, which is just one ticket id among N and misleads users).
    pub quick_prompt_id: Option<String>,
    pub quick_prompt_name: Option<String>,
    pub quick_prompt_icon: Option<String>,
    /// Parent linear workflow run id (None for top-level manual batches).
    pub parent_run_id: Option<String>,
    /// Name of the workflow that spawned this batch, resolved at query time.
    /// None when the parent run was deleted or this is a manual batch.
    pub parent_workflow_id: Option<String>,
    pub parent_workflow_name: Option<String>,
    /// 1-based position of the parent linear run among all runs of that
    /// workflow (ordered by started_at). None for manual batches.
    pub parent_run_sequence: Option<u32>,
}

// ─── Workflow suggestions ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowSuggestion {
    pub id: String,
    pub title: String,
    pub description: String,
    pub reason: String,
    pub required_mcps: Vec<String>,
    pub audience: String,
    pub complexity: String,
    pub trigger: WorkflowTrigger,
    pub steps: Vec<WorkflowStep>,
}

/// 0.7.0 UX pass — payload for `POST /api/workflows/import`.
/// `content` is the raw JSON of a `WorkflowExportEnvelope` (string
/// rather than nested object so the frontend can `JSON.parse` once
/// from the dropped file and pass it through). `project_id` is the
/// importer's project to attach the workflow to — `None` keeps it
/// unattached (the user picks a project later via the wizard).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportWorkflowRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// 0.7.0 UX pass — payload for `POST /api/quick-prompts/import`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickPromptRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// 0.6.0 UX pass — optional payload for `POST /api/workflows/:id/trigger`.
/// `variables` carries user-entered values matching `Workflow.variables`
/// (manual launch). The keys become `{{var_name}}` in step prompts via
/// the run's `trigger_context`. Empty/missing body keeps the legacy
/// "trigger with no variables" flow working — back-compat for tracker
/// triggers that don't need variables.
#[derive(Debug, Deserialize, Default, TS)]
#[ts(export)]
pub struct TriggerWorkflowRequest {
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
}

/// Self-contained envelope produced by `GET /api/workflows/:id/export`.
/// Designed to be saved to disk, mailed, attached to a Github issue, etc.
/// `version: 1` is the current shape; future incompatible changes bump
/// the version and add a migration path at import time.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowExportEnvelope {
    /// Discriminator: always `"kronn.workflow"` for this envelope.
    pub kind: String,
    /// Schema version. Bumped on incompatible changes.
    pub version: u32,
    /// ISO-8601 timestamp of the export, for audit and human readability.
    pub exported_at: DateTime<Utc>,
    /// The workflow definition. `id`, `project_id`, `created_at`,
    /// `updated_at`, `enabled` are kept in the wire format (so a
    /// roundtrip is lossless to inspect) but DROPPED at import — the
    /// importer mints fresh values for those fields.
    pub workflow: Workflow,
    /// QPs referenced by `BatchQuickPrompt` steps. Bundled so the
    /// importer doesn't need to fetch them separately. Empty when no
    /// step references a QP.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub referenced_quick_prompts: Vec<QuickPrompt>,
}

/// Self-contained envelope produced by `GET /api/quick-prompts/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickPromptExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// Like the workflow envelope: `id`, `project_id`, `created_at`,
    /// `updated_at` are present on the wire but reset at import.
    pub quick_prompt: QuickPrompt,
}

/// 0.6.0 — payload for `POST /api/quick-apis/import`. Mirrors the QP shape.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportQuickApiRequest {
    pub content: String,
    pub project_id: Option<String>,
}

/// Self-contained envelope produced by `GET /api/quick-apis/:id/export`.
#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuickApiExportEnvelope {
    pub kind: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// `id`, `project_id`, `created_at`, `updated_at` are present on the
    /// wire but reset at import — fresh values are minted by the importer.
    pub quick_api: QuickApi,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/run`. Lets the user
/// launch a saved QuickApi standalone (Run drawer in the Quick APIs page),
/// passing values for the declared `variables`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RunQuickApiRequest {
    /// Map of variable name → user-entered value. Keys must match
    /// `QuickApi.variables[*].name`. Missing keys for required variables
    /// get the call rejected before any HTTP fires.
    #[serde(default)]
    #[ts(type = "Record<string, string>")]
    pub variables: ::std::collections::HashMap<String, String>,
}

/// Response from `POST /api/quick-apis/:id/run`. Mirrors the
/// `/test-api-call` shape so the frontend can reuse the same UI.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct RunQuickApiResponse {
    pub success: bool,
    pub duration_ms: u64,
    /// Parsed envelope (data/status/summary) on success, `None` on failure.
    pub envelope: Option<serde_json::Value>,
    /// Error message on failure, `None` on success.
    pub error: Option<String>,
}

/// 0.6.0 — payload for `POST /api/quick-apis/:id/batch`. Fan-out the same
/// QA over a list of items (sub-domains, ticket keys, languages, etc.)
/// without needing a workflow. Mirror of the `BatchApiCall` step type
/// but standalone — uses the same parallel HTTP executor under the hood.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiRequest {
    /// Items to fan-out over. Accepts:
    ///   - JSON array of strings (each fills the QA's first variable):
    ///     `["www.example.com", "de.example.com", "fr.example.com"]`
    ///   - JSON array of objects (each key maps to a variable name):
    ///     `[{"host":"www.example.com","limit":"5"}, ...]`
    pub items: serde_json::Value,
    /// Max parallel HTTP calls (default 5, hard-capped at 20).
    #[serde(default)]
    pub concurrent_limit: Option<u32>,
}

/// Response from `POST /api/quick-apis/:id/batch`. The full aggregated
/// envelope produced by the BatchApiCall executor — the frontend renders
/// `envelope.data.items[]` as a per-item result table.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct BatchRunQuickApiResponse {
    /// Overall status: `OK` (all succeeded), `PARTIAL` (some failed), `ERROR` (all failed).
    pub status: String,
    pub duration_ms: u64,
    pub envelope: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct TestStepRequest {
    pub step: WorkflowStep,
    pub project_id: Option<String>,
    /// Mock previous step output (raw text or structured JSON)
    #[serde(default)]
    pub mock_previous_output: Option<String>,
    /// Additional mock variables: {"issue.title": "...", "steps.collect.data": "..."}
    #[serde(default)]
    pub mock_variables: Option<std::collections::HashMap<String, String>>,
    /// Dry run: agent describes what it would do without executing any write actions
    #[serde(default)]
    pub dry_run: bool,
}

// ─── MCP API requests ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateMcpConfigRequest {
    pub server_id: String,
    pub label: String,
    #[ts(type = "Record<string, string>")]
    pub env: std::collections::HashMap<String, String>,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub project_ids: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateMcpConfigRequest {
    pub label: Option<String>,
    #[ts(type = "Record<string, string> | null")]
    pub env: Option<std::collections::HashMap<String, String>>,
    pub args_override: Option<Vec<String>>,
    pub is_global: Option<bool>,
    pub include_general: Option<bool>,
    pub host_sync: Option<HostSyncMode>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct LinkMcpConfigRequest {
    pub project_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpOverview {
    pub servers: Vec<McpServer>,
    pub configs: Vec<McpConfigDisplay>,
    /// Set of "slug:projectId" pairs where the context file has been customized (not default template).
    #[serde(default)]
    pub customized_contexts: Vec<String>,
    /// Known incompatibilities between MCP servers and agents.
    #[serde(default)]
    pub incompatibilities: Vec<McpIncompatibility>,
}

/// A known incompatibility between an MCP server and a specific agent.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpIncompatibility {
    /// The MCP server ID (e.g. "mcp-gitlab", "detected:data-gouv-fr")
    pub server_id: String,
    /// The agent that is incompatible
    pub agent: AgentType,
    /// Human-readable explanation
    pub reason: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Audit Drift Detection
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriftCheckResponse {
    pub audit_date: Option<String>,
    pub stale_sections: Vec<DriftSection>,
    pub fresh_sections: Vec<String>,
    pub total_sections: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DriftSection {
    pub ai_file: String,
    pub audit_step: usize,
    pub changed_sources: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct PartialAuditRequest {
    pub agent: AgentType,
    pub steps: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StartBriefingResponse {
    pub discussion_id: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetBriefingRequest {
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateDiscussionRequest {
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentType,
    #[serde(default = "default_language")]
    pub language: String,
    pub initial_prompt: String,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
    #[serde(default)]
    pub workspace_mode: Option<String>,
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Model capability tier (economy / default / reasoning).
    #[serde(default)]
    pub tier: ModelTier,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateDiscussionRequest {
    pub title: Option<String>,
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub skill_ids: Option<Vec<String>>,
    pub profile_ids: Option<Vec<String>>,
    pub directive_ids: Option<Vec<String>>,
    /// Change project: Some(Some("id")) = set, Some(None) = unset, absent = no change
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Option<String>>,
    /// Change model tier for this discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
    /// Switch the primary agent for this discussion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentType>,
}

fn default_language() -> String {
    "fr".into()
}

// ═══════════════════════════════════════════════════════════════════════════════
// Ollama (local LLM) — 0.4.0
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OllamaModel {
    pub name: String,
    pub size: String,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaHealthResponse {
    /// "online", "offline", "not_installed", "unreachable"
    pub status: String,
    pub version: Option<String>,
    pub endpoint: String,
    pub models_count: u32,
    /// User-facing explanation when status != "online". Contextualized
    /// for the detected environment (native, Docker, WSL).
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaModelsResponse {
    pub models: Vec<OllamaModel>,
}

fn default_ui_language() -> String {
    "fr".into()
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub target_agent: Option<AgentType>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ShareDiscussionRequest {
    pub contact_ids: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct OrchestrationRequest {
    pub agents: Vec<AgentType>,
    pub max_rounds: Option<u32>,
    #[serde(default)]
    pub skill_ids: Vec<String>,
    #[serde(default)]
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub directive_ids: Vec<String>,
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
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP Context Files
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpContextEntry {
    pub slug: String,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct UpdateMcpContextRequest {
    pub content: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Database Info & Export/Import
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct DbInfo {
    pub size_bytes: u64,
    pub project_count: u32,
    pub discussion_count: u32,
    pub message_count: u32,
    pub mcp_count: u32,
    pub workflow_count: u32,
    pub workflow_run_count: u32,
    pub custom_skill_count: u32,
    pub custom_profile_count: u32,
    pub custom_directive_count: u32,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DbExport {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub projects: Vec<Project>,
    pub discussions: Vec<Discussion>,
    #[serde(default)]
    pub workflows: Vec<Workflow>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    #[serde(default)]
    pub mcp_configs: Vec<McpConfig>,
    #[serde(default)]
    pub custom_skills: Vec<Skill>,
    #[serde(default)]
    pub custom_directives: Vec<Directive>,
    #[serde(default)]
    pub custom_profiles: Vec<AgentProfile>,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub quick_prompts: Vec<QuickPrompt>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportResult {
    pub warnings: Vec<String>,
    pub invalid_paths: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Git Operations
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitStatusResponse {
    pub branch: String,
    pub default_branch: String,
    pub is_default_branch: bool,
    pub files: Vec<GitFileStatus>,
    pub ahead: u32,
    pub behind: u32,
    pub has_upstream: bool,
    pub provider: String,  // "github", "gitlab", or "unknown"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitFileStatus {
    pub path: String,
    pub status: String,
    pub staged: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitDiffResponse {
    pub path: String,
    pub diff: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitDiffQuery {
    pub path: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitBranchRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitBranchResponse {
    pub branch: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GitCommitRequest {
    pub files: Vec<String>,
    pub message: String,
    #[serde(default)]
    pub amend: bool,
    #[serde(default)]
    pub sign: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitCommitResponse {
    pub hash: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct GitPushResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatePrRequest {
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_pr_base")]
    pub base: String,
}

fn default_pr_base() -> String { "main".into() }

#[derive(Debug, Deserialize)]
pub struct ExecRequest {
    pub command: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ExecResponse {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self { success: true, data: Some(data), error: None }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self { success: false, data: None, error: Some(msg.into()) }
    }
}

/// Paginated API response — wraps a list with total count + page info.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
}

/// Query params for paginated endpoints.
///
/// IMPORTANT: `page` has NO serde default on purpose. The caller (e.g.
/// `api::discussions::list`) wraps this in `Option<Query<PaginationQuery>>`
/// and treats "no query params at all" as "return everything unpaginated".
/// If we gave `page` a default of 1, axum's extractor would always succeed
/// on a bare `GET /api/discussions` call, silently capping results at 50 —
/// that's the bug we hit on 2026-04-13 where users with >50 discussions
/// stopped seeing their older conversations once a 50-item batch ran.
#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_per_page() -> u32 { 50 }

// ═══════════════════════════════════════════════════════════════════════════════
// Context Files (uploaded file context for discussions)
// ═══════════════════════════════════════════════════════════════════════════════

/// A file uploaded as context for a discussion.
/// Content is extracted to text at upload time and stored in DB.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextFile {
    pub id: String,
    pub discussion_id: String,
    pub filename: String,
    pub mime_type: String,
    pub original_size: u64,
    pub extracted_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Response after uploading a context file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UploadContextFileResponse {
    pub file: ContextFile,
    /// Suggested skill IDs based on file extension
    pub suggested_skills: Vec<String>,
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
