//! 0.11.0 (KT-317) — durable multi-agent task-orchestration models.
//!
//! ADR-002 (`docs/design/adr-002-orchestration-multi-agent.md`) decides a
//! distinct `task_executions` aggregate (O2) that borrows the workflow engine's
//! proven invariants — sticky SQL-predicate transitions, boot reconcile,
//! terminal-lock — rather than folding a TaskExecution into `WorkflowRun`.
//!
//! This module holds the types + the pure state-machine logic (allowed
//! transitions, terminal set, and the §4bis integration-saga resume decision).
//! Persistence lives in `db/orchestration.rs`; the shared transition primitive
//! lives in `db/run_state.rs`. Provisioning (KT-318) and the protected Git merge
//! (KT-320) are deliberately not here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    AgentType, MessageTarget, MessageTargetKind, ModelTier, PlanningActor, PlanningDodItem,
    PlanningTaskSummary,
};

/// Versioned so downstream contract changes stay backward-compatible: new struct
/// fields are added with `#[serde(default)]`, and a consumer can branch on this.
pub const ORCHESTRATION_SCHEMA_VERSION: u32 = 1;

// ─── Enums ───────────────────────────────────────────────────────────────────

/// The V1 shape is one implicit run per launch (`single_task`); `campaign` is
/// reserved for KT-321 (a principal driving several ready tasks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OrchestrationRunKind {
    #[default]
    SingleTask,
    Campaign,
}

impl OrchestrationRunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleTask => "single_task",
            Self::Campaign => "campaign",
        }
    }
}

impl std::str::FromStr for OrchestrationRunKind {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "single_task" => Ok(Self::SingleTask),
            "campaign" => Ok(Self::Campaign),
            _ => anyhow::bail!("Unknown orchestration run kind: {value}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OrchestrationRunStatus {
    #[default]
    Active,
    Completed,
    Cancelled,
    Failed,
}

/// Operator-visible campaign state (KT-321). Kept distinct from the original
/// coarse lifecycle status so existing migration-127 databases remain readable:
/// `Paused` and `AwaitingHuman` are durable holds, not terminal outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum OrchestrationControlState {
    #[default]
    Running,
    Paused,
    AwaitingHuman,
    Completed,
    Cancelled,
    Failed,
}

impl OrchestrationControlState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::AwaitingHuman => "awaiting_human",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

impl std::str::FromStr for OrchestrationControlState {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "running" => Ok(Self::Running),
            "paused" => Ok(Self::Paused),
            "awaiting_human" => Ok(Self::AwaitingHuman),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => anyhow::bail!("Unknown orchestration control state: {value}"),
        }
    }
}

/// Exact default worker choice for a campaign. The typed target preserves the
/// identity (including an exact joined CLI session); model and profile remain
/// explicit, independently overrideable launch dimensions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CampaignWorkerSelection {
    pub target: MessageTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
}

impl OrchestrationRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

impl std::str::FromStr for OrchestrationRunStatus {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "active" => Ok(Self::Active),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => anyhow::bail!("Unknown orchestration run status: {value}"),
        }
    }
}

/// Only one strategy in V1 (ADR §4). Kept as an enum so KT-320+ can add variants
/// without a schema migration on the discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum IntegrationStrategy {
    /// Build+validate a candidate in an ephemeral worktree, then advance the
    /// parent fast-forward-only under lease + backup-ref + CAS.
    #[default]
    TwoPhaseFfOnly,
}

impl IntegrationStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TwoPhaseFfOnly => "two_phase_ff_only",
        }
    }
}

impl std::str::FromStr for IntegrationStrategy {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "two_phase_ff_only" => Ok(Self::TwoPhaseFfOnly),
            _ => anyhow::bail!("Unknown integration strategy: {value}"),
        }
    }
}

/// The TaskExecution state machine (ADR §3). Variant names are the exact DB
/// strings (PascalCase, mirroring `RunStatus`) so `as_str` and the migration
/// CHECK stay in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TaskExecutionStatus {
    Pending,
    Provisioning,
    Blocked,
    Working,
    AwaitingReview,
    Approved,
    ChangesRequested,
    Integrating,
    Validating,
    Applying,
    Escalated,
    Interrupted,
    Done,
    Failed,
    Cancelled,
}

impl TaskExecutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Provisioning => "Provisioning",
            Self::Blocked => "Blocked",
            Self::Working => "Working",
            Self::AwaitingReview => "AwaitingReview",
            Self::Approved => "Approved",
            Self::ChangesRequested => "ChangesRequested",
            Self::Integrating => "Integrating",
            Self::Validating => "Validating",
            Self::Applying => "Applying",
            Self::Escalated => "Escalated",
            Self::Interrupted => "Interrupted",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Canonical terminal set (ADR §3). Exhaustive match on purpose: adding a
    /// variant fails to compile here, so a new status cannot silently escape the
    /// state-machine guards. Only these three are sticky — a late/zombie worker
    /// snapshot can never resurrect them.
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Done | Self::Failed | Self::Cancelled => true,
            Self::Pending
            | Self::Provisioning
            | Self::Blocked
            | Self::Working
            | Self::AwaitingReview
            | Self::Approved
            | Self::ChangesRequested
            | Self::Integrating
            | Self::Validating
            | Self::Applying
            | Self::Escalated
            | Self::Interrupted => false,
        }
    }

    /// Is `to` a *structurally* legal successor of `self`? Encodes the ADR §3
    /// state machine. Three rules the §3 owner-table (l.278-281) states for
    /// *every* non-terminal state — rather than as per-arrow diagram entries —
    /// are generalized in the early return:
    ///   • any non-terminal → `Interrupted` (a backend restart hits any state);
    ///   • any non-terminal → `Cancelled` (user / principal abandon);
    ///   • any non-terminal → `Escalated` (backend enforces a budget/hard-fail).
    /// No transition ever leaves a terminal state (stickiness).
    ///
    /// This is the *coarse* gate. Two states have a durable resume checkpoint
    /// that further narrows the concrete target — and this pure `(self, to)`
    /// predicate cannot see it, so the narrowing lives in
    /// `db::orchestration::transition_execution` against the row:
    ///   • `Blocked` resumes ONLY to `blocked_from_status` (ADR §3 "clears back
    ///     to the state it left") — a Provisioning-origin Blocked must not jump
    ///     to Applying even though both are structurally allowed here;
    ///   • `Interrupted` is fully permissive here (any non-terminal target); the
    ///     concrete target is narrowed to `interrupted_from_status` (exact origin
    ///     or a legal successor of it) by the checkpoint guard, NOT by this arm.
    /// Use [`Self::blocked_resume_allowed`] / [`Self::interrupted_resume_allowed`]
    /// for the checkpoint-aware decision.
    pub fn can_transition_to(self, to: Self) -> bool {
        use TaskExecutionStatus::*;
        if self == to || self.is_terminal() {
            return false;
        }
        // ADR §3 owner-table generalizations (l.278-281): interrupt, cancel and
        // escalate reach any in-flight state, so they are not repeated per arm.
        if matches!(to, Interrupted | Cancelled | Escalated) {
            return true;
        }
        match self {
            Pending => matches!(to, Provisioning),
            Provisioning => matches!(to, Working | Blocked | Failed),
            // Structural gate only; the resume target is narrowed to
            // `blocked_from_status` by the checkpoint guard.
            Blocked => matches!(to, Provisioning | Applying),
            Working => matches!(to, AwaitingReview),
            AwaitingReview => matches!(to, Approved | ChangesRequested),
            Approved => matches!(to, Integrating),
            // `Provisioning` is the KT-319 rework re-offer path: a CLI worker must re-accept
            // a control offer before working the next attempt, so request_changes re-enters
            // the provisioning handshake — `ChangesRequested → Provisioning → Blocked
            // (awaiting_worker_acceptance) → Provisioning → Working` — the EXACT mirror of the
            // initial handshake, reusing its Provisioning-origin `Blocked` machinery so no new
            // `blocked_from_status` domain value (a frozen-127 CHECK) is needed. `Working`
            // remains the direct path (a native worker, which does not re-offer).
            ChangesRequested => matches!(to, Working | Provisioning),
            Integrating => matches!(to, Validating | ChangesRequested),
            Validating => matches!(to, Applying | ChangesRequested),
            Applying => matches!(to, Done | Integrating | Blocked),
            Escalated => matches!(to, Approved | Working),
            // Interrupted is the universal resume point: a backend restart can
            // interrupt ANY non-terminal state, so structurally it may resume ANY
            // non-terminal target. This coarse gate MUST NOT pre-filter — an
            // explicit list silently strands an interrupted AwaitingReview /
            // Approved / ChangesRequested / Pending outside Cancel/Escalate. The
            // REAL narrowing to the exact `interrupted_from_status` origin (or a
            // legal successor of that origin) is `interrupted_resume_allowed`,
            // applied against the row in `transition_execution`.
            Interrupted => !to.is_terminal(),
            // Terminal: handled by the early return above.
            Done | Failed | Cancelled => false,
        }
    }

    /// Checkpoint-aware resume guard for a `Blocked` row (ADR §3). A block clears
    /// back ONLY to the state it left (`blocked_from`); Cancel/Interrupt/Escalate
    /// are the generalized escapes and are handled by `can_transition_to`, not
    /// here. A Provisioning-origin block therefore cannot resume Applying. (The
    /// KT-319 rework re-offer parks a Provisioning-origin Blocked too — it re-enters
    /// Provisioning first — so it resumes to Provisioning like the initial handshake,
    /// needing no new rule here.)
    pub fn blocked_resume_allowed(blocked_from: Self, to: Self) -> bool {
        to == blocked_from
    }

    /// Checkpoint-aware resume guard for an `Interrupted` row (ADR §3, §4bis). It
    /// resumes to the exact state it left, or to any state that origin could
    /// legally advance to (the saga may redirect an interrupted `Applying` to
    /// `Integrating` on drift). Anchored on `interrupted_from`, never the coarse
    /// structural set — a Provisioning interrupt can never resume Applying. An
    /// interrupted `Blocked` re-enters `Blocked` only (never "sees through" it to
    /// bypass `blocked_from_status`).
    pub fn interrupted_resume_allowed(interrupted_from: Self, to: Self) -> bool {
        to == interrupted_from
            // A candidate built/validated against a parent tip that drifted is
            // intentionally discarded and rebuilt. Both Validating and Applying
            // therefore have one safe backwards saga edge to Integrating; it is
            // not a general business-state transition.
            || (matches!(interrupted_from, Self::Validating | Self::Applying)
                && to == Self::Integrating)
            || (interrupted_from != Self::Blocked && interrupted_from.can_transition_to(to))
    }
}

/// Lifecycle of a CLI worker control offer (KT-328). `pending` is the published,
/// unclaimed offer; `accepting` is the CAS-won intermediate held across the
/// two-phase session transfer (crash-safe resume target); `accepted` is the
/// committed handshake. `declined`/`expired`/`cancelled` are terminal non-accept
/// outcomes that free the execution for a re-offer or a native fallback. Variant
/// names serialize to the exact DB strings (snake_case) so `as_str` and the
/// migration CHECK stay in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkerOfferStatus {
    Pending,
    Accepting,
    Accepted,
    Declined,
    Expired,
    Cancelled,
}

impl WorkerOfferStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepting => "accepting",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    /// A live offer is one still open to acceptance (`pending`/`accepting`) — the
    /// exact set the partial unique indexes constrain. Exhaustive on purpose so a
    /// new variant must be classified here.
    pub fn is_live(self) -> bool {
        match self {
            Self::Pending | Self::Accepting => true,
            Self::Accepted | Self::Declined | Self::Expired | Self::Cancelled => false,
        }
    }
}

impl std::str::FromStr for WorkerOfferStatus {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "pending" => Ok(Self::Pending),
            "accepting" => Ok(Self::Accepting),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            _ => anyhow::bail!("Unknown worker offer status: {value}"),
        }
    }
}

impl std::str::FromStr for TaskExecutionStatus {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        use TaskExecutionStatus::*;
        Ok(match value {
            "Pending" => Pending,
            "Provisioning" => Provisioning,
            "Blocked" => Blocked,
            "Working" => Working,
            "AwaitingReview" => AwaitingReview,
            "Approved" => Approved,
            "ChangesRequested" => ChangesRequested,
            "Integrating" => Integrating,
            "Validating" => Validating,
            "Applying" => Applying,
            "Escalated" => Escalated,
            "Interrupted" => Interrupted,
            "Done" => Done,
            "Failed" => Failed,
            "Cancelled" => Cancelled,
            _ => anyhow::bail!("Unknown task execution status: {value}"),
        })
    }
}

/// Structured discriminant for a non-terminal `Blocked` TaskExecution (KT-328). A
/// `Blocked` row also carries a human-readable `blocked_reason`, but a consumer
/// classifies the hold on THIS code — never by matching prose (KT-334 owns the
/// attention-center split). New codes are added here without a schema migration (the
/// column has no SQL CHECK); the strict `FromStr` is the domain guard, so a corrupt
/// value surfaces instead of being silently coerced. Variant names serialize to the
/// exact DB strings (snake_case) so `as_str` and the enum stay in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BlockedReasonCode {
    /// A CLI worker control offer is published and the exact target session is
    /// expected to accept. NORMAL — not an attention item; the worker will act.
    AwaitingWorkerAcceptance,
    /// The target CLI session already holds a live offer for another execution.
    /// Needs a human decision: re-offer to another session or pick a native worker.
    WorkerSessionCommittedElsewhere,
}

impl BlockedReasonCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingWorkerAcceptance => "awaiting_worker_acceptance",
            Self::WorkerSessionCommittedElsewhere => "worker_session_committed_elsewhere",
        }
    }
}

impl std::str::FromStr for BlockedReasonCode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "awaiting_worker_acceptance" => Ok(Self::AwaitingWorkerAcceptance),
            "worker_session_committed_elsewhere" => Ok(Self::WorkerSessionCommittedElsewhere),
            _ => anyhow::bail!("Unknown blocked reason code: {value}"),
        }
    }
}

/// The action the §4bis boot saga takes for an in-flight integration, decided by
/// comparing durable checkpoints against the *real* parent tip. Pure logic so it
/// is testable without a Git repo (the actual ref reads + apply are KT-320/322).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SagaResumeAction {
    /// Candidate absent or built on a parent tip that has since drifted → throw
    /// it away and rebuild on the current tip (→ `Integrating`).
    RebuildCandidate,
    /// Candidate valid, verdict incomplete → re-run validations (→ `Validating`).
    RunValidations,
    /// Validated candidate, parent still at the anchor and clean → replay the
    /// fast-forward (→ `Applying`).
    ApplyFastForward,
    /// The parent already advanced to the candidate — the apply landed but the
    /// close didn't. Skip to the idempotent close (→ `Done`).
    IdempotentClose,
    /// Parent is at the anchor but dirty → hold (→ `Blocked`) until it is clean.
    BlockDirtyTarget,
    /// Fully applied (or nothing to do) → no-op.
    NoOp,
}

/// Durable decision produced by boot reconciliation. `Interrupted` remains the
/// visible execution status until this decision is applied, so merely opening
/// the database can never claim that a worker, review or Git mutation resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionRecoveryAction {
    ResumeProvisioning,
    ResumeWorker,
    AwaitReview,
    AwaitHuman,
    RebuildCandidate,
    RunValidations,
    ApplyFastForward,
    IdempotentClose,
    BlockDirtyTarget,
    BlockMissingWorkspace,
    BlockMissingDiscussion,
    BlockAgentUnavailable,
}

impl ExecutionRecoveryAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ResumeProvisioning => "resume_provisioning",
            Self::ResumeWorker => "resume_worker",
            Self::AwaitReview => "await_review",
            Self::AwaitHuman => "await_human",
            Self::RebuildCandidate => "rebuild_candidate",
            Self::RunValidations => "run_validations",
            Self::ApplyFastForward => "apply_fast_forward",
            Self::IdempotentClose => "idempotent_close",
            Self::BlockDirtyTarget => "block_dirty_target",
            Self::BlockMissingWorkspace => "block_missing_workspace",
            Self::BlockMissingDiscussion => "block_missing_discussion",
            Self::BlockAgentUnavailable => "block_agent_unavailable",
        }
    }
}

impl std::str::FromStr for ExecutionRecoveryAction {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "resume_provisioning" => Self::ResumeProvisioning,
            "resume_worker" => Self::ResumeWorker,
            "await_review" => Self::AwaitReview,
            "await_human" => Self::AwaitHuman,
            "rebuild_candidate" => Self::RebuildCandidate,
            "run_validations" => Self::RunValidations,
            "apply_fast_forward" => Self::ApplyFastForward,
            "idempotent_close" => Self::IdempotentClose,
            "block_dirty_target" => Self::BlockDirtyTarget,
            "block_missing_workspace" => Self::BlockMissingWorkspace,
            "block_missing_discussion" => Self::BlockMissingDiscussion,
            "block_agent_unavailable" => Self::BlockAgentUnavailable,
            other => anyhow::bail!("unknown execution recovery action: {other}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CancellationCleanupPolicy {
    Preserve,
    RemoveIfClean,
}

impl CancellationCleanupPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::RemoveIfClean => "remove_if_clean",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExecutionTimeoutKind {
    Activity,
    TotalDuration,
    ReviewWait,
    HumanWait,
}

/// Decide the resume action for a TaskExecution found mid-integration at boot
/// (ADR §4bis reconciliation table — the seven cases). `real_parent_tip` is the
/// actual `git rev-parse` of the target branch; `parent_dirty` its worktree
/// cleanliness. Returns `RebuildCandidate` for any ambiguous state — the safe
/// default is to rebuild rather than replay against an unknown tree.
pub fn saga_resume_action(
    status: TaskExecutionStatus,
    candidate_target_sha: Option<&str>,
    candidate_merge_sha: Option<&str>,
    integrated_sha: Option<&str>,
    real_parent_tip: Option<&str>,
    parent_dirty: bool,
) -> SagaResumeAction {
    use TaskExecutionStatus::*;
    let tip = real_parent_tip;
    match status {
        // Fully applied: the parent is at what we recorded → nothing to do.
        Done => {
            if integrated_sha.is_some() && tip == integrated_sha {
                SagaResumeAction::NoOp
            } else {
                // Recorded Done but the ref disagrees: never silently "fix" it.
                SagaResumeAction::NoOp
            }
        }
        Integrating => {
            if candidate_merge_sha.is_none() {
                // Candidate not built yet.
                SagaResumeAction::RebuildCandidate
            } else if tip.is_some() && tip == candidate_target_sha {
                SagaResumeAction::RunValidations
            } else {
                SagaResumeAction::RebuildCandidate
            }
        }
        Validating => {
            if candidate_merge_sha.is_none() {
                SagaResumeAction::RebuildCandidate
            } else if tip.is_some() && tip == candidate_target_sha {
                SagaResumeAction::RunValidations
            } else {
                // Parent drifted since the candidate was built.
                SagaResumeAction::RebuildCandidate
            }
        }
        Applying => {
            if integrated_sha.is_some() {
                // Apply already recorded → only the close may be pending.
                SagaResumeAction::IdempotentClose
            } else if tip.is_some() && tip == candidate_merge_sha {
                // Apply landed but integrated_sha wasn't recorded before crash.
                SagaResumeAction::IdempotentClose
            } else if tip.is_some() && tip == candidate_target_sha {
                if parent_dirty {
                    SagaResumeAction::BlockDirtyTarget
                } else {
                    SagaResumeAction::ApplyFastForward
                }
            } else {
                // Parent drifted → rebuild rather than force a stale candidate.
                SagaResumeAction::RebuildCandidate
            }
        }
        // Any other state has no half-applied integration to reconcile.
        _ => SagaResumeAction::NoOp,
    }
}

// ─── Persisted rows ────────────────────────────────────────────────────────

/// One validation to run on the candidate before the parent is advanced (§6).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(deny_unknown_fields)]
pub struct ValidationSpec {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_exec_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
}

/// The mandatory campaign envelope (ADR §1, §2).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OrchestrationRun {
    pub id: String,
    pub kind: OrchestrationRunKind,
    pub discussion_id: String,
    pub project_id: Option<String>,
    pub target_workspace_id: Option<String>,
    pub target_branch: Option<String>,
    pub max_review_rounds: u32,
    pub max_concurrent_executions: u32,
    pub token_budget: Option<u64>,
    pub integration_strategy: IntegrationStrategy,
    #[serde(default)]
    pub validations: Vec<ValidationSpec>,
    pub escalation_notify_url: Option<String>,
    /// DoD-7: the worker cannot self-approve BY DEFAULT (`false`). Only an explicit
    /// launcher opt-in (KT-321) sets this `true`, and only then may the execution's own
    /// worker identity decide its review — the exception is never a default.
    #[serde(default)]
    pub allow_self_review: bool,
    pub status: OrchestrationRunStatus,
    /// Durable principal/campaign control. A paused or human-gated run cannot
    /// launch work even though its coarse lifecycle is still `Active`.
    #[serde(default)]
    pub control_state: OrchestrationControlState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u32>,
    #[serde(default = "default_one")]
    pub max_cli_concurrent_executions: u32,
    #[serde(default)]
    pub allowed_agents: Vec<AgentType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_worker: Option<CampaignWorkerSelection>,
    #[serde(default)]
    pub auto_continue: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The durable unit of work (ADR §1, §3, §4bis).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecution {
    pub id: String,
    pub orchestration_run_id: String,
    pub task_id: String,
    pub parent_discussion_id: String,
    pub sub_discussion_id: Option<String>,
    pub workspace_id: Option<String>,
    pub dispatch_job_id: Option<String>,
    pub base_sha: Option<String>,
    pub child_branch: Option<String>,
    /// Worker identity — the durable typed `MessageTarget` contract (ADR §5), not
    /// a loose provider string. `worker_target_kind` mirrors [`MessageTargetKind`]
    /// and, for a `Cli` worker, `worker_cli_session_id` pins the exact joined
    /// session (two CLIs of one provider are never confused). All-or-nothing:
    /// a set kind requires `worker_agent_type`; `Cli` also requires the session id.
    /// All NULL until provisioning (KT-318) selects the worker.
    #[serde(default)]
    pub worker_target_kind: Option<MessageTargetKind>,
    #[serde(default)]
    pub worker_cli_session_id: Option<i64>,
    #[serde(default)]
    pub worker_connection_id: Option<String>,
    pub worker_agent_type: Option<String>,
    pub worker_model: Option<String>,
    pub worker_model_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_profile_id: Option<String>,
    /// Optional principal-authored mechanical scope for a deliberately tiny
    /// local-worker edit. It is persisted with the execution and interpreted
    /// by the runner; the worker cannot broaden it through prose or tool args.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_scope: Option<TaskWorkerScope>,
    /// Ordered DoD ids frozen when the execution is created. Native HTTP and
    /// spawned-host workers submit assertions by position, so delivery must
    /// refuse a task whose DoD was reordered/replaced under the active brief.
    /// `None` identifies executions created before migration 142.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_dod_ids: Option<Vec<String>>,
    /// Semantic brief/re-dispatch counter (ADR §5). 0 at launch; KT-319 bumps it
    /// per business attempt so brief/dispatch dedupe keys are attempt-scoped.
    #[serde(default)]
    pub attempt_no: u32,
    pub status: TaskExecutionStatus,
    /// Durable resume checkpoints (ADR §3). Set while the row is on a `Blocked`
    /// (→ the state to clear back to) or `Interrupted` (→ the exact state it left)
    /// hold; cleared once resumed. Consumed by the checkpoint guard in
    /// `db::orchestration::transition_execution`.
    #[serde(default)]
    pub blocked_from_status: Option<TaskExecutionStatus>,
    #[serde(default)]
    pub interrupted_from_status: Option<TaskExecutionStatus>,
    pub review_rounds: u32,
    pub max_review_rounds: u32,
    pub candidate_target_sha: Option<String>,
    pub candidate_merge_sha: Option<String>,
    pub integrated_sha: Option<String>,
    pub backup_ref: Option<String>,
    pub blocked_reason: Option<String>,
    /// Structured discriminant for the current `Blocked` hold (KT-328). Consumers
    /// branch on this, never on `blocked_reason` prose (KT-334). NULL for a native
    /// checkpoint-refused block in V1; set for the CLI control-offer holds.
    #[serde(default)]
    pub blocked_reason_code: Option<BlockedReasonCode>,
    pub outcome_reason: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// Machine-checkable scope for a worker that should not explore the repository.
///
/// The target is deliberately only a path plus the minimum coordinates needed
/// by the selected mechanical mutation. The CAS receipt is always produced by
/// a real `read_file` inside the pinned worktree after provisioning; a
/// principal-supplied hash would be stale authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[ts(export)]
pub enum TaskWorkerScope {
    PrelocalizedEdit {
        path: String,
        start_line: u32,
        end_line: u32,
    },
    /// Insert new text immediately after one frozen line without allowing the
    /// worker to replace or delete that anchor. The file receipt is obtained
    /// from the provisioned worktree, exactly like `PrelocalizedEdit`.
    PrelocalizedInsertAfter { path: String, anchor_line: u32 },
}

impl TaskWorkerScope {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::PrelocalizedEdit {
                path,
                start_line,
                end_line,
            } => {
                let trimmed = path.trim();
                if trimmed.is_empty() {
                    return Err("worker_scope.path must be non-empty".into());
                }
                let candidate = std::path::Path::new(trimmed);
                if candidate.is_absolute()
                    || candidate
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(
                        "worker_scope.path must stay relative to the managed worktree".into(),
                    );
                }
                if *start_line == 0 || *end_line == 0 || start_line > end_line {
                    return Err(
                        "worker_scope line range must be positive, inclusive and ordered".into(),
                    );
                }
                if end_line.saturating_sub(*start_line) >= 200 {
                    return Err(
                        "worker_scope prelocalized range cannot exceed 200 inclusive lines".into(),
                    );
                }
                Ok(())
            }
            Self::PrelocalizedInsertAfter { path, anchor_line } => {
                let trimmed = path.trim();
                if trimmed.is_empty() {
                    return Err("worker_scope.path must be non-empty".into());
                }
                let candidate = std::path::Path::new(trimmed);
                if candidate.is_absolute()
                    || candidate
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(
                        "worker_scope.path must stay relative to the managed worktree".into(),
                    );
                }
                if *anchor_line == 0 {
                    return Err("worker_scope anchor_line must be positive".into());
                }
                Ok(())
            }
        }
    }
}

/// Recovery projection kept separately from the business state machine.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionRecovery {
    pub task_execution_id: String,
    pub recovery_action: ExecutionRecoveryAction,
    pub recovery_reason: String,
    pub last_activity_at: DateTime<Utc>,
    pub total_deadline_at: Option<DateTime<Utc>>,
    pub activity_deadline_at: Option<DateTime<Utc>>,
    pub review_deadline_at: Option<DateTime<Utc>>,
    pub human_wait_started_at: Option<DateTime<Utc>>,
    pub assignment_generation: u32,
    pub watchdog_redispatches: u32,
    pub pending: bool,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OrchestrationResiliencePolicy {
    pub activity_timeout_secs: Option<u32>,
    pub review_timeout_secs: Option<u32>,
    pub human_wait_timeout_secs: Option<u32>,
    pub cancellation_cleanup_policy: CancellationCleanupPolicy,
}

impl Default for OrchestrationResiliencePolicy {
    fn default() -> Self {
        Self {
            activity_timeout_secs: None,
            review_timeout_secs: None,
            human_wait_timeout_secs: None,
            // KT-514 — a task keeps at most one live execution, so every prior
            // attempt of a relaunched task is already terminal. Preserving each
            // cancelled checkout by default is what let 18 worktrees pile up on a
            // single task until the sandbox guard blocked every worker. The
            // default now reclaims the clean checkout (the branch and its commits
            // survive — `git worktree remove` never touches the ref); an operator
            // who wants the working tree kept for inspection still opts into
            // `Preserve` explicitly.
            cancellation_cleanup_policy: CancellationCleanupPolicy::RemoveIfClean,
        }
    }
}

/// One journaled transition (ADR §3; DoD-3).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionEvent {
    pub id: String,
    pub task_execution_id: String,
    pub action: String,
    pub from_status: Option<TaskExecutionStatus>,
    pub to_status: Option<TaskExecutionStatus>,
    pub actor_kind: super::PlanningActorKind,
    pub actor_id: Option<String>,
    pub actor_session_id: Option<String>,
    #[ts(type = "any")]
    pub changes: serde_json::Value,
    pub source_message_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One recorded validation run (ADR §6). `exit_code` IS the verdict.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionValidationRun {
    pub id: String,
    pub task_execution_id: String,
    pub candidate_merge_sha: Option<String>,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub output: Option<String>,
    pub quick_exec_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl TaskExecutionValidationRun {
    /// A run passes only on an explicit exit 0. A NULL exit (process died / never
    /// started) is never a pass — this is what closes "exit 0 always".
    pub fn passed(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// A durable CLI worker control offer (KT-328). Links a TaskExecution + attempt to
/// the exact target session, the origin room it was posted in, and the
/// sub-discussion it grants access to. The `id` is the opaque handle an agent
/// accepts by — never a raw kr-join token.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionWorkerOffer {
    pub id: String,
    pub task_execution_id: String,
    pub attempt_no: u32,
    pub target_cli_session_id: i64,
    pub origin_discussion_id: String,
    pub child_discussion_id: String,
    pub status: WorkerOfferStatus,
    pub expires_at: Option<DateTime<Utc>>,
    pub offer_message_id: Option<String>,
    pub reason: Option<String>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub declined_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A persisted worker delivery (128, KT-319 DoD-1): the exact validated DeliveryManifest
/// bytes plus the denormalized `head_sha` (so the DoD-5 drift check is a column read, not
/// a JSON extract), one row per `(execution, attempt)`. Backend-facing (serde-only, not
/// `#[ts(export)]`); KT-323/334 can export it when the front renders deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionDelivery {
    pub id: String,
    pub task_execution_id: String,
    pub attempt_no: u32,
    pub head_sha: String,
    pub manifest_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A persisted principal review (128, KT-319 DoD-2/8): the exact validated ReviewDecision
/// bytes and its `decision` discriminant, one row per `(execution, attempt)` — a re-decide
/// of the same attempt upserts (idempotent), a request_changes bumps the attempt so the
/// next decision lands on its own auditable row. Backend-facing (serde-only, not
/// `#[ts(export)]`); KT-323/334 can export it when the front renders reviews.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionReview {
    pub id: String,
    pub task_execution_id: String,
    pub attempt_no: u32,
    pub decision: String,
    pub decision_json: String,
    pub created_at: String,
    pub updated_at: String,
}

// ─── Inputs ────────────────────────────────────────────────────────────────

/// Policy for an OrchestrationRun. Defaults give a V1 single-task run.
#[derive(Debug, Clone)]
pub struct OrchestrationRunInput {
    pub kind: OrchestrationRunKind,
    pub discussion_id: String,
    pub project_id: Option<String>,
    pub target_workspace_id: Option<String>,
    pub target_branch: Option<String>,
    pub max_review_rounds: u32,
    pub max_concurrent_executions: u32,
    pub token_budget: Option<u64>,
    pub integration_strategy: IntegrationStrategy,
    pub validations: Vec<ValidationSpec>,
    pub escalation_notify_url: Option<String>,
    pub timeout_secs: Option<u32>,
    pub max_cli_concurrent_executions: u32,
    pub allowed_agents: Vec<AgentType>,
    pub default_worker: Option<CampaignWorkerSelection>,
    pub auto_continue: bool,
}

impl OrchestrationRunInput {
    /// A minimal single-task run rooted at the given principal discussion.
    pub fn single_task(discussion_id: impl Into<String>) -> Self {
        Self {
            kind: OrchestrationRunKind::SingleTask,
            discussion_id: discussion_id.into(),
            project_id: None,
            target_workspace_id: None,
            target_branch: None,
            max_review_rounds: 3,
            max_concurrent_executions: 1,
            token_budget: None,
            integration_strategy: IntegrationStrategy::TwoPhaseFfOnly,
            validations: Vec::new(),
            escalation_notify_url: None,
            timeout_secs: None,
            max_cli_concurrent_executions: 1,
            allowed_agents: Vec::new(),
            default_worker: None,
            auto_continue: false,
        }
    }
}

/// The parameters of a single-task "Create and run" launch (ADR §1).
#[derive(Debug, Clone)]
pub struct LaunchSingleTaskInput {
    pub task_id: String,
    pub parent_discussion_id: String,
    pub project_id: Option<String>,
    pub target_workspace_id: Option<String>,
    pub target_branch: Option<String>,
    pub base_sha: Option<String>,
    pub child_branch: Option<String>,
    /// Worker identity (ADR §5). `worker_target_kind` = None keeps the KT-317
    /// behaviour (a loose `worker_agent_type` only); provisioning (KT-318) sets the
    /// exact typed identity. For a `Cli` kind, `worker_cli_session_id` is required.
    pub worker_target_kind: Option<MessageTargetKind>,
    pub worker_cli_session_id: Option<i64>,
    pub worker_connection_id: Option<String>,
    pub worker_agent_type: Option<String>,
    pub worker_model: Option<String>,
    pub worker_model_tier: Option<String>,
    pub worker_profile_id: Option<String>,
    pub worker_scope: Option<TaskWorkerScope>,
    pub worker_dod_ids: Option<Vec<String>>,
    pub max_review_rounds: u32,
    /// Principal-authored mechanical gates for the implicit single-task run.
    /// These are never accepted from the worker delivery manifest.
    pub validations: Vec<ValidationSpec>,
    /// A retry with the same key returns the existing execution — no duplicate.
    pub idempotency_key: Option<String>,
}

impl LaunchSingleTaskInput {
    pub fn new(task_id: impl Into<String>, parent_discussion_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            parent_discussion_id: parent_discussion_id.into(),
            project_id: None,
            target_workspace_id: None,
            target_branch: None,
            base_sha: None,
            child_branch: None,
            worker_target_kind: None,
            worker_cli_session_id: None,
            worker_connection_id: None,
            worker_agent_type: None,
            worker_model: None,
            worker_model_tier: None,
            worker_profile_id: None,
            worker_scope: None,
            worker_dod_ids: None,
            max_review_rounds: 3,
            validations: Vec::new(),
            idempotency_key: None,
        }
    }
}

fn default_one() -> u32 {
    1
}

/// Stable reasons why a plan entry cannot be selected by a campaign. Codes are
/// for clients; `detail` stays readable for agents and humans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CampaignTaskReason {
    pub code: String,
    pub detail: String,
}

/// One tier the native worker target can receive, with the model Kronn would
/// resolve today. `None` is meaningful for host CLIs whose own account-aware
/// default remains authoritative; HTTP providers need a concrete model before
/// the catalogue can call them available.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskWorkerTier {
    pub tier: ModelTier,
    pub resolved_model: Option<String>,
}

/// A worker identity that can be copied verbatim into `task_exec_prepare`.
///
/// `configured` and `reachable` are independent observations. The only strict
/// implication is `available => configured && reachable`; for example a saved
/// provider can be temporarily down, while NVIDIA's public catalogue can be
/// reachable before the account key is configured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskWorkerCatalogueEntry {
    pub worker: MessageTarget,
    pub label: String,
    /// Model declared by an exact joined CLI. Native targets expose their
    /// tier-to-model resolution in `tiers` instead.
    pub declared_model: Option<String>,
    pub configured: bool,
    pub reachable: bool,
    pub available: bool,
    #[serde(default)]
    pub tiers: Vec<TaskWorkerTier>,
    #[serde(default)]
    pub reasons: Vec<CampaignTaskReason>,
    #[serde(default)]
    pub warnings: Vec<CampaignTaskReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskWorkerCatalogue {
    pub workers: Vec<TaskWorkerCatalogueEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CampaignTaskCandidate {
    pub task: PlanningTaskSummary,
    pub plan_position: u32,
    pub launchable: bool,
    #[serde(default)]
    pub reasons: Vec<CampaignTaskReason>,
}

/// What the principal currently owns. This makes the coordinator inspectable
/// instead of an opaque loop hidden behind an "active" badge.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PrincipalAttention {
    pub active_executions: u32,
    pub cli_executions: u32,
    pub awaiting_review: u32,
    pub awaiting_human: u32,
    pub ready_tasks: u32,
    #[serde(default)]
    pub actions: Vec<String>,
}

/// A resolved TaskExecution + its lineage, answerable in one query (DoD-4):
/// OrchestrationRun → TaskExecution → task → parent/sub-discussion → workspace,
/// without rebuilding the chain from chat messages.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionLineage {
    pub execution: TaskExecution,
    pub orchestration_run_kind: OrchestrationRunKind,
    pub task_reference: String,
    pub task_title: String,
    pub parent_discussion_id: String,
    pub sub_discussion_id: Option<String>,
    pub workspace_canonical_path: Option<String>,
}

/// What one execution consumed. In-app replies and joined CLI sessions are kept
/// separate because the former is a per-message counter while the latter is a
/// whole-session running total. Unknown CLI telemetry remains `None`, never 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskExecutionHttpPhase {
    Read,
    Mutation,
    Commit,
    Delivery,
    Finalization,
    Exploration,
    Answer,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionHttpToolUsage {
    pub name: String,
    pub ok: bool,
}

/// Secret-free provider accounting for one HTTP model response. Tool names are
/// bounded protocol identifiers; arguments, results, prompts and endpoints are
/// deliberately absent from this durable projection.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionHttpTurnUsage {
    pub turn: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    pub provider: String,
    pub phase: TaskExecutionHttpPhase,
    pub prompt_tokens: u64,
    pub eval_tokens: u64,
    pub duration_ms: u64,
    pub provider_ok: bool,
    pub requested_tools: Vec<String>,
    pub executed_tools: Vec<TaskExecutionHttpToolUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionHttpPhaseUsage {
    pub phase: TaskExecutionHttpPhase,
    pub turns: u32,
    pub prompt_tokens: u64,
    pub eval_tokens: u64,
    pub duration_ms: u64,
}

/// Aggregate across every dispatch/rework of one durable task execution. The
/// totals cover the complete journal while `recent_turns` is bounded for UI and
/// MCP payload safety.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionHttpUsage {
    pub turns: u32,
    pub prompt_tokens: u64,
    pub eval_tokens: u64,
    pub traffic_tokens: u64,
    pub peak_context_tokens: u64,
    pub duration_ms: u64,
    pub phases: Vec<TaskExecutionHttpPhaseUsage>,
    pub recent_turns: Vec<TaskExecutionHttpTurnUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionUsage {
    pub duration_ms: i64,
    pub in_app_tokens: i64,
    pub in_app_messages: i64,
    pub in_app_cost_usd: Option<f64>,
    pub in_app_cost_is_partial: bool,
    pub cli_traffic_tokens: Option<i64>,
    pub cli_billable_tokens: Option<i64>,
    pub cli_sessions: i64,
    pub cli_sessions_measured: i64,
    pub cli_sessions_unmeasured: i64,
    pub http: Option<TaskExecutionHttpUsage>,
}

/// Time spent in one state. The status enum bounds label cardinality to the
/// state-machine domain; callers never receive a reason or another free-form
/// string as a metric label.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionStateDuration {
    pub status: TaskExecutionStatus,
    pub duration_ms: i64,
}

/// A bounded counter used by the execution observability projection. `code` is
/// produced only from closed enums / fixed constants in the service layer, never
/// copied from user or agent prose.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionMetricCount {
    pub code: String,
    pub count: u32,
}

/// Operational metrics for one execution. Usage keeps unknown CLI telemetry as
/// `None`; state and reason labels remain bounded so this projection can safely
/// feed dashboards without a cardinality explosion.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionMetrics {
    pub state_durations: Vec<TaskExecutionStateDuration>,
    pub waiting_duration_ms: i64,
    pub review_rounds: u32,
    pub attempt_count: u32,
    pub validation_failures: u32,
    pub failures: Vec<TaskExecutionMetricCount>,
    pub blocking_reasons: Vec<TaskExecutionMetricCount>,
    pub usage: TaskExecutionUsage,
}

/// Redacted journal row. The persisted event may contain arbitrary comments,
/// validation output or reasons in `changes_json`; none of that crosses this
/// observability boundary. Durable correlation and attribution IDs remain.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionAuditEvent {
    pub id: String,
    pub action: String,
    pub from_status: Option<TaskExecutionStatus>,
    pub to_status: Option<TaskExecutionStatus>,
    pub actor_kind: super::PlanningActorKind,
    pub actor_id: Option<String>,
    pub actor_session_id: Option<String>,
    pub source_message_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One-call, secret-free observability surface. `lineage.execution` supplies the
/// durable run/task/discussion/workspace/dispatch IDs, while `audit_events`
/// supplies the ordered attributed history without raw payloads.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionObservability {
    pub lineage: TaskExecutionLineage,
    pub metrics: TaskExecutionMetrics,
    pub audit_events: Vec<TaskExecutionAuditEvent>,
}

/// Delivery/review pair for one semantic worker attempt. Keeping every attempt
/// makes review ping-pong inspectable instead of replacing its history with the
/// latest answer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionAttemptDetail {
    pub attempt_no: u32,
    pub delivery: Option<DeliveryManifestV1>,
    pub review: Option<ReviewDecisionV1>,
}

/// One-call projection for the execution detail UI (KT-323). Durable state stays
/// sourced from the orchestration aggregate; task DoD, manifests, validations
/// and telemetry are joined here so clients never reconstruct lineage from chat.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionDetail {
    pub lineage: TaskExecutionLineage,
    pub target_branch: Option<String>,
    pub definition_of_done: Vec<PlanningDodItem>,
    pub attempts: Vec<TaskExecutionAttemptDetail>,
    pub validation_runs: Vec<TaskExecutionValidationRun>,
    pub recovery: Option<TaskExecutionRecovery>,
    pub usage: TaskExecutionUsage,
    pub progress: TaskExecutionProgress,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskExecutionProgressPhase {
    Queued,
    Launching,
    UpstreamWait,
    ToolActivity,
    Delivering,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskExecutionTelemetryMode {
    BoundaryOnly,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionProgress {
    pub phase: TaskExecutionProgressPhase,
    pub reason: Option<String>,
    pub queue_position: Option<u32>,
    pub queued_since: Option<DateTime<Utc>>,
    pub process_alive: Option<bool>,
    pub last_reliable_signal_at: Option<DateTime<Utc>>,
    pub telemetry_mode: TaskExecutionTelemetryMode,
}

/// Compact sidebar edge. The canonical relation already lives on
/// `task_executions`; this projection avoids overloading the workflow-run FK on
/// discussions or making clients fetch every execution detail separately.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecutionDiscussionLink {
    pub execution_id: String,
    pub orchestration_run_id: String,
    pub task_id: String,
    pub task_reference: String,
    pub task_title: String,
    pub parent_discussion_id: String,
    pub sub_discussion_id: String,
    pub status: TaskExecutionStatus,
}

/// Read-only launch preflight for agent surfaces. Every refusal is a stable
/// code + actionable detail; calling it never creates a run or worktree.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskExecutionPreparation {
    pub task: super::PlanningTaskDetail,
    pub parent_discussion_id: String,
    pub worker: MessageTarget,
    pub project_id: Option<String>,
    pub launchable: bool,
    pub reasons: Vec<CampaignTaskReason>,
    pub active_execution: Option<TaskExecution>,
}

/// A single-task launch result: the execution plus whether the idempotent key
/// matched an existing row (so callers can tell a fresh launch from a replay).
#[derive(Debug, Clone)]
pub struct LaunchOutcome {
    pub run: OrchestrationRun,
    pub execution: TaskExecution,
    /// `true` when an idempotency-key match returned the existing execution.
    pub deduplicated: bool,
}

/// The versioned, backward-compatible wire response for a single-task launch —
/// the V1 contract KT-318's endpoint will return (no endpoint is wired in
/// KT-317). `schema_version` lets a consumer branch on the orchestration schema;
/// new fields are added later with `#[serde(default)]`. Built from the internal
/// `LaunchOutcome`. Serde/typegen tested here so KT-318 can expose it unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaunchTaskExecutionResponseV1 {
    /// Always [`ORCHESTRATION_SCHEMA_VERSION`]; pins the response contract.
    pub schema_version: u32,
    pub run: OrchestrationRun,
    pub execution: TaskExecution,
    /// `true` when an idempotency-key match returned the existing execution.
    pub deduplicated: bool,
}

impl From<LaunchOutcome> for LaunchTaskExecutionResponseV1 {
    fn from(outcome: LaunchOutcome) -> Self {
        Self {
            schema_version: ORCHESTRATION_SCHEMA_VERSION,
            run: outcome.run,
            execution: outcome.execution,
            deduplicated: outcome.deduplicated,
        }
    }
}

/// Re-export the shared actor type so orchestration event writers use the same
/// non-spoofable, server-injected identity as planning writes.
pub type OrchestrationActor = PlanningActor;

// ─── KT-319 structured delivery + review contracts (ADR §5) ──────────────────
//
// The versioned DeliveryManifest / ReviewDecision of the durable review loop.
// They are validated with the same JSON-subset `TypedSchema` validator as the
// triage manifest (`workflows::template::validate_envelope_against_schema`); the
// parse+validate glue lives in `api::orchestration` (service layer) so this
// module keeps no workflow dependency. Persisted attempt-scoped in migration 128.
//
// These are agent/bridge-facing content contracts (submitted via the
// `task_exec_deliver` / `task_exec_review` tools), NOT frontend wire types, so
// they are intentionally serde-only (no `#[ts(export)]`, no typegen). Wiring them
// to a typed frontend surface is KT-323 / KT-334.

/// Contract version pinned by every [`DeliveryManifestV1`]. `delivery_manifest_v1_schema`
/// consumes this constant, so it is the single source of truth: bumping it moves the
/// accepted `version` with it, with no separate literal to keep in sync.
pub const DELIVERY_CONTRACT_VERSION: &str = "1";

/// Contract version pinned by every [`ReviewDecisionV1`]. Versioned independently of the
/// delivery contract (the review schema can evolve on its own) and consumed by
/// `review_decision_v1_schema` — again the single source of truth, no bare literal.
pub const REVIEW_CONTRACT_VERSION: &str = "1";

/// How a file changed, in a [`DeliveryManifestV1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One touched file in a [`DeliveryManifestV1`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ManifestFile {
    pub path: String,
    pub kind: FileChangeKind,
}

/// A test verdict in a [`DeliveryManifestV1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Pass,
    Fail,
    Skipped,
}

/// One reported test in a [`DeliveryManifestV1`]. `evidence` is a `path:line` or
/// the command that proves the status; the reviewer (not the schema) refuses a
/// green without evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ManifestTest {
    pub name: String,
    pub status: TestStatus,
    #[serde(default)]
    pub evidence: Option<String>,
}

/// A per-DoD coverage claim. `dod_id` references the task's DoD item; the schema
/// only enforces shape — the approve guard (DoD-5, tranche 3) enforces coverage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ManifestDodStatus {
    pub dod_id: String,
    pub met: bool,
    #[serde(default)]
    pub evidence: Option<String>,
}

/// Worker → backend/principal delivery (ADR §5, KT-319 DoD-1). Every field DoD-1
/// enumerates is REQUIRED: `docs` / `migrations` / `risks` / `limitations` are
/// present even when empty, so "no migrations" is an explicit `[]`, never a
/// silent omission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeliveryManifestV1 {
    pub version: String,
    pub task_ref: String,
    pub head_sha: String,
    pub files_touched: Vec<ManifestFile>,
    pub tests: Vec<ManifestTest>,
    pub dod_status: Vec<ManifestDodStatus>,
    pub docs: Vec<String>,
    pub migrations: Vec<String>,
    pub risks: Vec<String>,
    pub limitations: Vec<String>,
    pub summary: String,
}

/// JSON-subset schema for [`DeliveryManifestV1`] (triage precedent). Required
/// fields mirror the struct; the four list fields are required arrays (possibly
/// empty) so DoD-1's "contient au minimum …" holds.
pub fn delivery_manifest_v1_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": [
            "version", "task_ref", "head_sha", "files_touched", "tests",
            "dod_status", "docs", "migrations", "risks", "limitations", "summary"
        ],
        "properties": {
            "version": { "type": "string", "enum": [DELIVERY_CONTRACT_VERSION] },
            "task_ref": { "type": "string", "minLength": 1 },
            // Abbreviated shas are legitimate git revs; kept at 7 so a worker is never
            // false-refused at parse. DoD-5 (tranche 3) normalizes BOTH this head_sha
            // and the live worktree HEAD through `resolve_commit` before comparing, so
            // short-vs-long can never trigger a spurious drift refusal.
            "head_sha": { "type": "string", "minLength": 7 },
            "files_touched": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["path", "kind"],
                    "properties": {
                        "path": { "type": "string", "minLength": 1 },
                        "kind": { "type": "string", "enum": ["added", "modified", "deleted"] }
                    }
                }
            },
            "tests": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "status"],
                    "properties": {
                        "name": { "type": "string", "minLength": 1 },
                        "status": { "type": "string", "enum": ["pass", "fail", "skipped"] },
                        "evidence": { "type": "string" }
                    }
                }
            },
            "dod_status": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["dod_id", "met"],
                    "properties": {
                        "dod_id": { "type": "string", "minLength": 1 },
                        "met": { "type": "boolean" },
                        "evidence": { "type": "string" }
                    }
                }
            },
            "docs": { "type": "array", "items": { "type": "string" } },
            "migrations": { "type": "array", "items": { "type": "string" } },
            "risks": { "type": "array", "items": { "type": "string" } },
            "limitations": { "type": "array", "items": { "type": "string" } },
            "summary": { "type": "string", "minLength": 1 }
        }
    })
}

/// The principal's verdict on a delivered attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
}

/// A structured review finding (ADR §5). `issue` is required; `path` / `line`
/// locate it when applicable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewFinding {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    pub issue: String,
}

/// Evidence produced by the authorized reviewer for a DoD item the worker
/// could not verify itself (most often a shell test on an HTTP worker). It is
/// persisted inside the attempt-scoped ReviewDecision, never inferred from a
/// mutable Planning checkbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewDodVerification {
    pub dod_id: String,
    pub met: bool,
    pub evidence: String,
}

/// Principal → backend review (ADR §5, KT-319). `comment` is schema-optional but
/// REQUIRED when `decision == request_changes` (a change request with no reason
/// is not actionable — DoD-4); that conditional is enforced by the parse layer,
/// not expressible in the JSON subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewDecisionV1 {
    pub version: String,
    pub task_ref: String,
    pub decision: ReviewVerdict,
    /// Exact delivered HEAD the reviewer inspected. Required by the parse
    /// layer for approval; optional for request_changes, whose findings may be
    /// issued before the reviewer finishes every validation.
    #[serde(default)]
    pub reviewed_head_sha: Option<String>,
    /// Principal-owned evidence for worker-unmet DoD items, bound durably to
    /// this review's execution attempt and reviewed HEAD.
    #[serde(default)]
    pub dod_verifications: Vec<ReviewDodVerification>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub findings: Vec<ReviewFinding>,
}

/// JSON-subset schema for [`ReviewDecisionV1`].
pub fn review_decision_v1_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["version", "task_ref", "decision"],
        "properties": {
            "version": { "type": "string", "enum": [REVIEW_CONTRACT_VERSION] },
            "task_ref": { "type": "string", "minLength": 1 },
            "decision": { "type": "string", "enum": ["approve", "request_changes"] },
            "reviewed_head_sha": { "type": "string", "minLength": 7 },
            "dod_verifications": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["dod_id", "met", "evidence"],
                    "properties": {
                        "dod_id": { "type": "string", "minLength": 1 },
                        "met": { "type": "boolean" },
                        "evidence": { "type": "string", "minLength": 1 }
                    }
                }
            },
            "comment": { "type": "string" },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["issue"],
                    "properties": {
                        "path": { "type": "string" },
                        "line": { "type": "integer" },
                        "issue": { "type": "string", "minLength": 1 }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::TaskExecutionStatus::{self, *};
    use super::TaskWorkerScope;

    /// KT-514 — a cancellation with no explicit policy must reclaim the clean
    /// checkout (branch preserved) rather than hoard it. This default is what
    /// stops a relaunched task from stacking one worktree per attempt.
    #[test]
    fn default_cancellation_policy_reclaims_the_clean_checkout() {
        assert_eq!(
            super::OrchestrationResiliencePolicy::default().cancellation_cleanup_policy,
            super::CancellationCleanupPolicy::RemoveIfClean,
        );
    }

    #[test]
    fn prelocalized_worker_scope_is_small_relative_and_inclusive() {
        assert!(TaskWorkerScope::PrelocalizedEdit {
            path: "backend/src/lib.rs".into(),
            start_line: 10,
            end_line: 209,
        }
        .validate()
        .is_ok());
        for invalid in [
            TaskWorkerScope::PrelocalizedEdit {
                path: "../outside.rs".into(),
                start_line: 10,
                end_line: 20,
            },
            TaskWorkerScope::PrelocalizedEdit {
                path: "/absolute.rs".into(),
                start_line: 10,
                end_line: 20,
            },
            TaskWorkerScope::PrelocalizedEdit {
                path: "src/too-large.rs".into(),
                start_line: 10,
                end_line: 210,
            },
            TaskWorkerScope::PrelocalizedEdit {
                path: "src/reversed.rs".into(),
                start_line: 20,
                end_line: 10,
            },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?} must be refused");
        }

        assert!(TaskWorkerScope::PrelocalizedInsertAfter {
            path: "docs/guide.md".into(),
            anchor_line: 42,
        }
        .validate()
        .is_ok());
        for invalid in [
            TaskWorkerScope::PrelocalizedInsertAfter {
                path: "../outside.md".into(),
                anchor_line: 42,
            },
            TaskWorkerScope::PrelocalizedInsertAfter {
                path: "/absolute.md".into(),
                anchor_line: 42,
            },
            TaskWorkerScope::PrelocalizedInsertAfter {
                path: "docs/guide.md".into(),
                anchor_line: 0,
            },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?} must be refused");
        }
    }

    /// ADR §3 rework arc (KT-319 tranche 3b): request_changes re-enters the provisioning
    /// handshake (`ChangesRequested → Provisioning → Blocked → Provisioning → Working`) — the
    /// exact mirror of the initial handshake, so it needs only the one new `ChangesRequested →
    /// Provisioning` arc and reuses the Provisioning-origin `Blocked` resume unchanged.
    #[test]
    fn request_changes_re_enters_the_provisioning_handshake_for_the_next_attempt() {
        // The one new arc: request_changes re-provisions the worker for the next attempt.
        assert!(
            ChangesRequested.can_transition_to(Provisioning),
            "rework re-enters provisioning"
        );
        assert!(
            ChangesRequested.can_transition_to(Working),
            "the direct path (native) stays legal"
        );
        // The reused initial-handshake path.
        assert!(
            Provisioning.can_transition_to(Blocked),
            "the re-offer parks Blocked"
        );
        assert!(
            Blocked.can_transition_to(Provisioning),
            "the re-accept resumes to Provisioning"
        );
        assert!(
            Provisioning.can_transition_to(Working),
            "then Provisioning → Working"
        );
        // A rework Blocked is Provisioning-origin, so it resumes exactly like the initial
        // handshake — the guard still forbids skipping straight to Working.
        assert!(TaskExecutionStatus::blocked_resume_allowed(
            Provisioning,
            Provisioning
        ));
        assert!(
            !TaskExecutionStatus::blocked_resume_allowed(Provisioning, Working),
            "a Blocked hold never skips its origin to reach Working directly"
        );
        // request_changes still cannot reach a terminal/review state directly.
        assert!(
            !ChangesRequested.can_transition_to(Blocked),
            "it goes via Provisioning, not straight to Blocked"
        );
        assert!(!ChangesRequested.can_transition_to(Approved));
    }
}
