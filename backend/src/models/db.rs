// Database snapshot + export/import payloads. `DbInfo` is the small
// header the Settings page renders; `DbExport` is the full self-contained
// dump consumed by the import wizard with path remapping.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    AgentProfile, Contact, Directive, Discussion, Learning, LearningRejection, McpConfig,
    McpServer, Project, QuickApi, QuickPrompt, QuickPromptVersion, Skill, Workflow,
};

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

/// Current export schema version. Bump when a new table/field is added to
/// `DbExport` so import can WARN when restoring an older backup (whose missing
/// tables must NOT wipe newer data — see `do_import_db`'s selective clear).
pub const CURRENT_EXPORT_VERSION: u32 = 5;

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
    /// 0.8.9 — Quick APIs (reusable saved API calls). `#[serde(default)]` keeps
    /// v3 exports (which had no `quick_apis` field) importable.
    #[serde(default)]
    pub quick_apis: Vec<QuickApi>,
    /// 0.8.9 — Continual-learning candidates (the agent-proposed durable
    /// facts/preferences). Same back-compat default as `quick_apis`.
    #[serde(default)]
    pub learnings: Vec<Learning>,
    /// v5 (passe D) — QP version history; without it, imports silently lost
    /// the version metrics lineage. `default` keeps v4 exports importable.
    #[serde(default)]
    pub quick_prompt_versions: Vec<QuickPromptVersion>,
    /// v5 (passe D) — anti-repetition rejection counters for learnings.
    #[serde(default)]
    pub learning_rejections: Vec<LearningRejection>,
}

#[derive(Debug, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportResult {
    pub warnings: Vec<String>,
    pub invalid_paths: Vec<String>,
}
