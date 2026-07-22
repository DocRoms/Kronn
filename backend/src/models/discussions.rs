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
    /// Subset of `message_count` excluding `MessageRole::System` rows. The
    /// streaming layer persists every tool call + every cached-summary
    /// breadcrumb as its own System message, so `message_count` is inflated
    /// from the user's point of view ("2 réponses + 50 outils" comptait 52).
    /// The unread badge tracks this count instead, so System breadcrumbs
    /// don't show up as "messages à lire".
    #[serde(default)]
    pub non_system_message_count: u32,
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
    /// 0.8.10 — explicit model override for this discussion (e.g. inherited
    /// from the Quick Prompt that launched it). Wins over `tier` at run time
    /// (threaded to the agent as `model_override`). `None` = resolve from tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
    /// The disc is owed an agent run that hasn't produced a durable trace yet
    /// (queued batch child, or a reply in flight). DB-backed so the sidebar's
    /// "en file" state survives navigation, reloads and missed WS frames.
    #[serde(default)]
    pub awaiting_agent: bool,
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
    // 0.8.4 (#294) — cross-agent memory source binding intentionally
    // NOT exposed on this struct. The columns
    // `source_agent / source_session_id / imported_at / diverged_at`
    // exist on the `discussions` table (migration 054) as a fast
    // "current source pointer" but are read through dedicated DB
    // helpers + a sibling `DiscussionSource` struct in
    // `db::disc_source_history`. Keeping `Discussion` lean avoids
    // breaking 50+ test fixtures + every code site that constructs
    // a discussion (~30 sites). The full link history lives in the
    // append-only `disc_source_history` table.
}

fn default_workspace_mode() -> String {
    "Direct".into()
}

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
    /// 0.8.10 — the CONCRETE model this message ran on (e.g. "qwen3:32b",
    /// "sonnet"), resolved via `runner::effective_model_flag` at commit time.
    /// A discussion can switch models mid-thread, so this is per-message, not
    /// per-discussion. `None` = legacy row or a provider-default run with no
    /// explicit model flag (Codex/Gemini at default tier) → UI falls back to
    /// `model_tier` / the agent name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Cost in USD (real from Claude Code, estimated for other providers)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Author identity (for multi-user / display)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_pseudo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_avatar_email: Option<String>,
    /// 0.8.4 (#294) — when this message came from a CLI transcript
    /// import, the source-side message id. Used by `disc_append` to
    /// dedupe re-pushes of the same exported transcript. NULL = native
    /// Kronn message (created via the UI / API, not imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_msg_id: Option<String>,
    /// 0.8.5 — wall-clock duration of the agent reply, in milliseconds.
    /// Captured by the streaming layer (delta between agent run start
    /// and message commit). NULL on User / System messages and on
    /// legacy rows (pre-migration 057). Used by the QP-metrics
    /// aggregator to compute "avg first-reply duration" per QP version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// 0.8.7 anti-hallucination P2 — the lint report for this agent message
    /// (niveau 0 heuristic + niveau 1 mechanical `[src:]` verification),
    /// computed by `core::anti_halluc::analyze` at finalize. `None` on
    /// User/System messages, when the feature is off, or when nothing flagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lint_report: Option<crate::core::anti_halluc::LintReport>,
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

impl SummaryStrategy {
    /// Whether the background auto-summary should fire, given the GLOBAL default
    /// (`ServerConfig::default_summary_strategy`, the Settings toggle) and THIS
    /// disc's stored strategy.
    ///
    /// The global `Off` is a **master kill-switch**: turning auto-summary off in
    /// Settings suppresses it everywhere, including older discs whose per-disc
    /// strategy was frozen to `Auto` at creation (the global default is only
    /// applied to NEW discs, so changing it never rewrote existing rows — the
    /// "I disabled it but long discs keep summarising" bug). Otherwise the
    /// per-disc strategy decides, and only `Auto` auto-fires.
    pub fn auto_fires(global_default: SummaryStrategy, disc: SummaryStrategy) -> bool {
        if matches!(global_default, SummaryStrategy::Off) {
            return false;
        }
        matches!(disc, SummaryStrategy::Auto)
    }
}

#[cfg(test)]
mod summary_strategy_tests {
    use super::SummaryStrategy;
    use super::SummaryStrategy::{Auto, Off, OnDemand};

    #[test]
    fn global_off_is_a_master_kill_switch() {
        // The reported bug: global Off must suppress even an old disc frozen to Auto.
        assert!(!SummaryStrategy::auto_fires(Off, Auto));
        assert!(!SummaryStrategy::auto_fires(Off, OnDemand));
        assert!(!SummaryStrategy::auto_fires(Off, Off));
    }

    #[test]
    fn per_disc_decides_when_global_is_not_off() {
        // Global Auto (or OnDemand) → the per-disc strategy is honoured.
        assert!(SummaryStrategy::auto_fires(Auto, Auto));
        assert!(!SummaryStrategy::auto_fires(Auto, Off));
        assert!(!SummaryStrategy::auto_fires(Auto, OnDemand));
        assert!(SummaryStrategy::auto_fires(OnDemand, Auto));
        assert!(!SummaryStrategy::auto_fires(OnDemand, Off));
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MessageRole {
    // `User` is the default so `#[serde(default)]` on federated frames from an
    // older peer (no `role` field on the wire) decodes to the historical
    // behaviour (every federated message used to land as User).
    #[default]
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
    /// 0.8.5 — when this discussion is being spawned by a Quick Prompt
    /// launch (single, batch, or compare-agents path that bypasses
    /// `create_batch_run`), the originating QP id. The backend
    /// resolves the current version_index and stamps both on the
    /// `discussions` row so the metrics aggregator can group.
    /// `None` = not a QP launch (briefing / manual / etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub originating_qp_id: Option<String>,
    /// F9 — create a "human-only" disc: the agent runner never spawns on
    /// `send_message`. Used by the contact-click → 1:1 human↔human chat flow.
    #[serde(default)]
    pub no_agent: bool,
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
