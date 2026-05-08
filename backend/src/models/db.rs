// Database snapshot + export/import payloads. `DbInfo` is the small
// header the Settings page renders; `DbExport` is the full self-contained
// dump consumed by the import wizard with path remapping.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    AgentProfile, Contact, Directive, Discussion, McpConfig, McpServer, Project, QuickPrompt,
    Skill, Workflow,
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
