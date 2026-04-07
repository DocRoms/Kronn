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
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub disabled_agents: Vec<AgentType>,
    #[serde(default)]
    #[ts(skip)]
    pub encryption_secret: Option<String>,
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
}

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
    }

    /// Returns true if at least one agent is marked as installed.
    pub fn any_installed(&self) -> bool {
        self.claude_code.installed
            || self.codex.installed
            || self.gemini_cli.installed
            || self.kiro.installed
            || self.vibe.installed
            || self.copilot_cli.installed
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

/// An MCP server type (e.g. "GitHub", "Atlassian", "Context7")
/// Represents the abstract definition — command + args template.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub source: McpSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
    Streamable { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum McpSource {
    Registry,
    Detected,
    Manual,
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
}

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
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepOutputFormat {
    #[default]
    FreeText,
    Structured,
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
}

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
    Goto { step_name: String },
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    WaitingApproval,
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

#[allow(dead_code)]
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct ImportWorkflowRequest {
    pub content: String,
    pub project_id: Option<String>,
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
}

#[derive(Debug, Deserialize)]
pub struct UpdateServerConfigRequest {
    pub domain: Option<String>,
    pub max_concurrent_agents: Option<usize>,
    pub agent_stall_timeout_min: Option<u64>,
    pub pseudo: Option<String>,
    pub avatar_email: Option<String>,
    pub bio: Option<String>,
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
#[derive(Debug, Deserialize)]
pub struct PaginationQuery {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 { 1 }
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
