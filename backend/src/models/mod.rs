use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ═══════════════════════════════════════════════════════════════════════════════
// Setup & Configuration
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[ts(skip)]
    pub value: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ApiKeyDisplay {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub masked_value: String,
    pub active: bool,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct SaveApiKeyRequest {
    pub id: Option<String>,
    pub name: String,
    pub provider: String,
    pub value: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ApiKeysResponse {
    pub keys: Vec<ApiKeyDisplay>,
    pub disabled_overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ScanConfig {
    pub paths: Vec<String>,
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AgentsConfig {
    pub claude_code: AgentConfig,
    pub codex: AgentConfig,
    #[serde(default)]
    pub gemini_cli: AgentConfig,
}

impl AgentsConfig {
    /// Get the full_access setting for a given agent type.
    pub fn full_access_for(&self, agent: &AgentType) -> bool {
        match agent {
            AgentType::ClaudeCode => self.claude_code.full_access,
            AgentType::Codex => self.codex.full_access,
            AgentType::GeminiCli => self.gemini_cli.full_access,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AgentConfig {
    pub path: Option<String>,
    #[serde(default)]
    pub installed: bool,
    pub version: Option<String>,
    #[serde(default)]
    pub full_access: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Setup Wizard
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct SetupStatus {
    pub is_first_run: bool,
    pub current_step: SetupStep,
    pub agents_detected: Vec<AgentDetection>,
    pub scan_paths_set: bool,
    pub repos_detected: Vec<DetectedRepo>,
    pub default_scan_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum SetupStep {
    Agents,
    ScanPaths,
    Detection,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum AgentType {
    ClaudeCode,
    Codex,
    Vibe,
    GeminiCli,
    Custom,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Projects & Repositories
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct TokenOverride {
    pub provider: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AiConfigStatus {
    pub detected: bool,
    pub configs: Vec<AiConfigType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum AiAuditStatus {
    NoTemplate,
    TemplateInstalled,
    Audited,
    Validated,
}

impl Default for AiAuditStatus {
    fn default() -> Self {
        AiAuditStatus::NoTemplate
    }
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct LaunchAuditRequest {
    pub agent: AgentType,
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP (Model Context Protocol) — New 3-tier model
// ═══════════════════════════════════════════════════════════════════════════════

/// An MCP server type (e.g. "GitHub", "Atlassian", "Context7")
/// Represents the abstract definition — command + args template.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub description: String,
    pub transport: McpTransport,
    pub source: McpSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Sse { url: String },
    Streamable { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum McpSource {
    Registry,
    Detected,
    Manual,
}

/// A configured instance of an MCP server — with label, env secrets, etc.
/// Multiple projects can share the same config (deduplication by config_hash).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpConfig {
    pub id: String,
    pub server_id: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_encrypted: String,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
}

/// Display-safe version of McpConfig (secrets masked)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpConfigDisplay {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub label: String,
    pub env_keys: Vec<String>,
    pub env_masked: Vec<McpEnvEntry>,
    pub args_override: Option<Vec<String>>,
    pub is_global: bool,
    pub config_hash: String,
    pub project_ids: Vec<String>,
    pub project_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpEnvEntry {
    pub key: String,
    pub masked_value: String,
}

/// Registry entry — an MCP available for installation
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
}

// ═══════════════════════════════════════════════════════════════════════════════
// Workflows (replaces scheduled tasks)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
#[serde(tag = "type")]
pub enum TrackerSourceConfig {
    GitHub {
        owner: String,
        repo: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct WorkflowStep {
    pub name: String,
    pub agent: AgentType,
    pub prompt_template: String,
    pub mode: StepMode,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
#[serde(tag = "type")]
pub enum StepMode {
    Normal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AgentSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct StepConditionRule {
    pub contains: String,
    pub action: ConditionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
#[serde(tag = "type")]
pub enum ConditionAction {
    Stop,
    Skip,
    Goto { step_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct RetryConfig {
    pub max_retries: u32,
    #[serde(default = "default_backoff")]
    pub backoff: String,
}

fn default_backoff() -> String {
    "exponential".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub hooks: WorkspaceHooks,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum RunStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    WaitingApproval,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
// Stats & Analytics
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct TokenUsageSummary {
    pub total_tokens: u64,
    pub by_provider: Vec<ProviderUsage>,
    pub by_project: Vec<ProjectUsage>,
    pub daily_history: Vec<DailyUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ProviderUsage {
    pub provider: String,
    pub tokens_used: u64,
    pub tokens_limit: Option<u64>,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ProjectUsage {
    pub project_id: String,
    pub project_name: String,
    pub tokens_used: u64,
    pub task_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AgentUsageSummary {
    pub agent_type: String,
    pub total_tokens: u64,
    pub message_count: u32,
    pub by_project: Vec<AgentProjectUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct AgentProjectUsage {
    pub project_id: String,
    pub project_name: String,
    pub tokens_used: u64,
    pub message_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct DailyUsage {
    pub date: String,
    pub anthropic: u64,
    pub openai: u64,
    pub mistral: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Discussions
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct Discussion {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentType,
    pub language: String,
    pub participants: Vec<AgentType>,
    pub messages: Vec<DiscussionMessage>,
    #[serde(default)]
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub enum MessageRole {
    User,
    Agent,
    System,
}

// ═══════════════════════════════════════════════════════════════════════════════
// API Request/Response types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct SetScanPathsRequest {
    pub paths: Vec<String>,
}

// ─── Workflow API requests ────────────────────────────────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    pub trigger: Option<WorkflowTrigger>,
    pub steps: Option<Vec<WorkflowStep>>,
    pub actions: Option<Vec<WorkflowAction>>,
    pub safety: Option<WorkflowSafety>,
    pub workspace_config: Option<WorkspaceConfig>,
    pub concurrency_limit: Option<u32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct WorkflowRunSummary {
    pub id: String,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub tokens_used: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct ImportWorkflowRequest {
    pub content: String,
    pub project_id: Option<String>,
}

// ─── MCP API requests ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
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
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct UpdateMcpConfigRequest {
    pub label: Option<String>,
    #[ts(type = "Record<string, string> | null")]
    pub env: Option<std::collections::HashMap<String, String>>,
    pub args_override: Option<Vec<String>>,
    pub is_global: Option<bool>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct LinkMcpConfigRequest {
    pub project_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpOverview {
    pub servers: Vec<McpServer>,
    pub configs: Vec<McpConfigDisplay>,
    /// Set of "slug:projectId" pairs where the context file has been customized (not default template).
    #[serde(default)]
    pub customized_contexts: Vec<String>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct CreateDiscussionRequest {
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentType,
    #[serde(default = "default_language")]
    pub language: String,
    pub initial_prompt: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct UpdateDiscussionRequest {
    pub title: Option<String>,
    pub archived: Option<bool>,
}

fn default_language() -> String {
    "fr".into()
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub target_agent: Option<AgentType>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct OrchestrationRequest {
    pub agents: Vec<AgentType>,
    pub max_rounds: Option<u32>,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct SetAgentAccessRequest {
    pub agent: AgentType,
    pub full_access: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP Context Files
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct McpContextEntry {
    pub slug: String,
    pub label: String,
    pub content: String,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct UpdateMcpContextRequest {
    pub content: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Database Info & Export/Import
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Serialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct DbInfo {
    pub size_bytes: u64,
    pub project_count: u32,
    pub discussion_count: u32,
    pub message_count: u32,
    pub mcp_count: u32,
    pub task_count: u32,
    pub workflow_count: u32,
    pub workflow_run_count: u32,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../frontend/src/types/generated.ts")]
pub struct DbExport {
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub projects: Vec<Project>,
    pub discussions: Vec<Discussion>,
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
