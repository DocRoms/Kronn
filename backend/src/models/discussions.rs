// Discussion threads + their messages, plus the API request shapes used to
// create / update / interact with them. The "send a message" / "share with
// peer" / "orchestrate multiple agents" requests live here too because they
// always target a discussion.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

use super::{AgentType, ModelTier};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActiveAgentDispatch {
    pub id: String,
    pub trigger_message_id: String,
    pub agent_type: AgentType,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionDetail {
    #[serde(flatten)]
    pub discussion: Discussion,
    pub active_agent_dispatches: Vec<ActiveAgentDispatch>,
    /// Durable routing intent keyed by the user-message id. Keeping it next
    /// to the transcript lets the UI show what was requested even when the
    /// concrete model that eventually answered differs.
    #[serde(default)]
    pub message_targets: HashMap<String, Vec<MessageTarget>>,
}

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

/// Skip a `false` on the wire: a fragment is the exception, and paying a field on
/// every ordinary message would work against the release this lands in.
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionMessage {
    pub id: String,
    pub role: MessageRole,
    #[serde(default)]
    pub channel: MessageChannel,
    pub content: String,
    pub agent_type: Option<AgentType>,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub tokens_used: u64,
    /// KT-190 — what the JOINED CLI SESSION had spent by the time this message
    /// was posted. A running total, NOT this message's cost, and it lives apart
    /// from `tokens_used` for that reason: a CLI's spend cannot be cut per
    /// message, because between two room messages it also reads files, runs
    /// tests, and may answer in another room. So the UI must label it
    /// differently ("session: 220k"), never render it where a per-message cost
    /// goes.
    ///
    /// `None` for an agent Kronn spawned (its cost IS `tokens_used`) and for a
    /// CLI whose vendor has no collector — where absence means unmeasured, never
    /// free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_tokens_at_message: Option<i64>,
    /// KT-251 — `true` when this is the salvaged BEGINNING of an answer whose
    /// agent was killed mid-sentence, not an answer.
    ///
    /// Exposed so the UI can FOLD it rather than show it as a peer's position.
    /// Nothing is deleted: a fragment is real history and may hold reasoning the
    /// retry never repeated — a user reported seeing "three agents" precisely
    /// because two half-answers looked like real ones.
    #[serde(default, skip_serializing_if = "is_false")]
    pub recovered_partial: bool,
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
    /// 0.9.2 (KT-58) — the agent this message was explicitly dispatched to via a
    /// structured `@agent` mention. Written by the same transaction that
    /// enqueues the dispatch job, so the read model can distinguish "names an
    /// agent" from "awaits that agent's reply". `None` = ordinary prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_agent: Option<AgentType>,
    /// 0.9.2 (KT-73) — durable message this one answers. The referenced
    /// message must belong to the same discussion. Portable imports remap the
    /// identifier alongside message ids.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<String>,
    /// KT-247 — stable ordinal of the joined CLI session that authored this
    /// message (`1`, `2`, `3`…), resolved through `message_cli_authors`. Lets
    /// the header show "CLI 2" so two same-provider CLIs (two Claude Code) are
    /// distinguishable in the timeline, matching the `@claude-cli-2` alias.
    /// `None` for User/System/native-agent messages and legacy rows with no
    /// recorded CLI author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_cli_ordinal: Option<i64>,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MessageChannel {
    #[default]
    Main,
    Note,
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
    /// Explicit recipients of the initial message, including per-agent tier
    /// overrides selected from the new-discussion composer.
    #[serde(default)]
    pub initial_targets: Vec<MessageTarget>,
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
    /// Disable or restore Kronn's native fallback for this discussion. Joined
    /// peers remain participants and continue receiving turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_agent: Option<bool>,
    /// Disable generated agent-to-agent handoffs for this discussion even
    /// when the global opt-in is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_handoffs_disabled: Option<bool>,
    /// Remove the financial quota for this discussion only. The global master
    /// switch, per-agent blocks and structural loop guards still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_handoffs_unlimited: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionNativeAgentMode {
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiscussionAgentHandoffMode {
    pub global_enabled: bool,
    pub disabled: bool,
    pub unlimited_override: bool,
    pub effective_enabled: bool,
    /// `None` means no financial quota; structural loop guards still apply.
    pub paid_limit: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MessageTargetKind {
    DiscussionAgent,
    Agent,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageTarget {
    pub kind: MessageTargetKind,
    pub agent_type: AgentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_session_id: Option<i64>,
    /// Optional per-turn tier override. `None` preserves the historical
    /// discussion-wide routing; joined CLI sessions always ignore this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<ModelTier>,
}

impl MessageTarget {
    pub fn discussion_agent(agent_type: AgentType) -> Self {
        Self {
            kind: MessageTargetKind::DiscussionAgent,
            agent_type,
            cli_session_id: None,
            tier: None,
        }
    }

    pub fn agent(agent_type: AgentType) -> Self {
        Self {
            kind: MessageTargetKind::Agent,
            agent_type,
            cli_session_id: None,
            tier: None,
        }
    }

    pub fn cli(agent_type: AgentType, cli_session_id: i64) -> Self {
        Self {
            kind: MessageTargetKind::Cli,
            agent_type,
            cli_session_id: Some(cli_session_id),
            tier: None,
        }
    }

    pub fn with_tier(mut self, tier: ModelTier) -> Self {
        self.tier = Some(tier);
        self
    }
}

#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SendMessageRequest {
    pub content: String,
    /// Notes remain visible in the human timeline but never wake or enter an
    /// agent's context unless an explicit note-reading tool is used.
    #[serde(default)]
    pub channel: MessageChannel,
    /// Durable target identities selected by current clients. Unlike the
    /// compatibility `target_agents` projection, this distinguishes the
    /// configured discussion agent, a one-shot agent, and a joined CLI.
    #[serde(default)]
    pub targets: Vec<MessageTarget>,
    /// Expand to every responder already visible in the discussion: the
    /// configured agent, previously-addressed one-shot agents, and non-left
    /// joined CLI sessions. It never means every installed agent.
    #[serde(default)]
    pub target_all: bool,
    /// Every explicit `@agent` addressee, deduplicated in textual order.
    /// `target_agent` below remains accepted for older clients.
    #[serde(default)]
    pub target_agents: Vec<AgentType>,
    #[serde(default)]
    pub target_agent: Option<AgentType>,
    #[serde(default)]
    pub client_message_id: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
}

/// Atomic edit/resend request. `expected_revision` is the opaque timestamp
/// exposed on the target message. `idempotency_key` is generated once by the
/// UI and reused when the same HTTP request is retried.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct ReviseMessageRequest {
    pub message_id: String,
    pub content: String,
    pub expected_revision: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub targets: Vec<MessageTarget>,
    #[serde(default)]
    pub target_all: bool,
    #[serde(default)]
    pub target_agent: Option<AgentType>,
    /// Plural replacement for `target_agent`; empty preserves legacy clients.
    #[serde(default)]
    pub target_agents: Vec<AgentType>,
}

#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(export)]
pub struct RunAgentRequest {
    /// Stable per-click key. Reusing it after a transport retry returns the
    /// original durable obligation instead of launching a second agent turn.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MessageRevisionReceipt {
    pub event_id: String,
    pub message_id: String,
    pub revision: String,
    pub sort_order: i64,
    pub duplicate: bool,
    pub dispatch_job_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageRevisionEvent {
    pub id: String,
    pub discussion_id: String,
    pub target_message_id: String,
    pub previous_content_hash: String,
    pub expected_revision: String,
    pub revision: String,
    pub content: String,
    pub target_agent: Option<AgentType>,
    pub idempotency_key: String,
    pub sort_order: i64,
    pub dispatch_job_id: Option<String>,
    pub created_at: DateTime<Utc>,
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
