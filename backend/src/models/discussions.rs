// Discussion threads + their messages, plus the API request shapes used to
// create / update / interact with them. The "send a message" / "share with
// peer" / "orchestrate multiple agents" requests live here too because they
// always target a discussion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentType, ModelTier};

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
    /// How summaries are produced for this discussion. See `SummaryStrategy`
    /// for the semantics. Default `Auto` keeps the historical behaviour
    /// (per-agent thresholds with auto-fire after every reply).
    #[serde(default)]
    pub summary_strategy: SummaryStrategy,
    /// Cumulative count of `kronn-internal` tool calls made by the agent
    /// on this discussion. Bumped each time `disc_meta`, `disc_get_message`
    /// or `disc_summarize` is hit. Surfaced in the ChatHeader as a small
    /// "🔧 N" pill so the user can see when the agent is actively
    /// querying its history.
    #[serde(default)]
    pub introspection_call_count: u32,
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

/// Per-discussion summary strategy. Pre-fix the auto-summary loop fired
/// after every agent reply once a per-agent threshold was crossed (12/8/4
/// non-system messages). For big-context models or short threads that's
/// often a waste — user feedback on 2026-05-09 asked for an off switch.
///
/// `OnDemand` is reserved for the future kronn-internal MCP tool surface
/// (`disc_summarize` callable by the agent itself); for now it behaves
/// like `Off` from the auto-fire perspective and only differs in that we
/// keep the cache mechanism alive so an explicit summarize call updates
/// `summary_cache`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SummaryStrategy {
    /// Fire after every reply when the per-agent threshold is crossed.
    /// Default for backward compatibility.
    #[default]
    Auto,
    /// No auto-fire. Reserved for the planned introspection tool surface
    /// where the agent decides if/when to summarise.
    OnDemand,
    /// Never summarise. The agent receives the raw transcript until its
    /// context window saturates. Suitable for big-context models on
    /// short-to-medium threads, or when token cost matters more than
    /// context completeness.
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MessageRole {
    User,
    Agent,
    System,
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateDiscussionRequest {
    pub project_id: Option<String>,
    pub title: String,
    pub agent: AgentType,
    #[serde(default = "super::setup::default_language")]
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
    /// Change the auto-summary policy. Persists in `discussions.summary_strategy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_strategy: Option<SummaryStrategy>,
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
