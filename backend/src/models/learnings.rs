//! 0.9.0 — Continual Learning data model.
//!
//! A `Learning` is an agent-proposed durable fact / preference / inference,
//! emitted via the typed MCP tool `learning_propose`, gated by evidence
//! verification (Gate-1 existence + Gate-2 faithfulness) + a human, then
//! promoted into a dedicated learnings file. See
//! `docs/research/continual-learning-0.9.0-spec.md`.
//!
//! Enums serialize to the exact lowercase strings stored in the DB (snake_case)
//! so the row<->struct mapping in `db/learnings.rs` is a 1:1 string round-trip.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Type of a learning. Bound to anti-hallu `SourceKind` at the gate
/// (spec §5): `fact` needs a Verified file/url, `preference` a dated user
/// confirmation, `inference` is Unchecked → never auto-extracted to a truth
/// file without double validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LearningKind {
    Fact,
    Preference,
    Inference,
}

/// Lifecycle of a learning candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LearningStatus {
    Pending,
    Validated,
    Rejected,
    Stale,
    Promoted,
    /// Transient claim during validation: a row goes `pending → promoting` (CAS)
    /// BEFORE the file write, then `promoting → promoted` on success (or back to
    /// `pending` if the write fails). Blocks concurrent validate/reject races.
    Promoting,
}

/// Where a validated learning is routed (spec §7). `User` → `~/.kronn/
/// user-context/learnings.md`; `Project` → `docs/learnings.md`. NULL until the
/// scope router runs at validation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum LearningScope {
    User,
    Project,
}

/// Gate-2 faithfulness verdict (`claim ⊨ evidence`). NULL when the checker is
/// `off`. Posture B: informative only — surfaced to the human, never auto-blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Faithfulness {
    Entailment,
    Neutral,
    Contradiction,
}

/// One piece of evidence backing a claim. `kind` mirrors the citable source
/// types; `reference` is the resolvable ref (file:line / url / disc-id / cmd /
/// user:date); `quote` is the supporting excerpt (the NL premise the Gate-2
/// checker scores against).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Evidence {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

/// One row of `learning_rejections` — the anti-repetition counter keyed by
/// claim hash. Exported/imported with the DB (passe D: losing it reset the
/// auto-reject threshold after a migration).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LearningRejection {
    pub claim_hash: String,
    pub reason: String,
    pub count: i64,
    pub last_at: String,
}

/// A continual-learning candidate (table `learnings`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Learning {
    pub id: String,
    pub claim: String,
    pub evidence: Vec<Evidence>,
    pub kind: LearningKind,
    pub status: LearningStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<LearningScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faithfulness: Option<Faithfulness>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discussion_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_target: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_validated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validated_by: Option<String>,
}

impl LearningKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            LearningKind::Fact => "fact",
            LearningKind::Preference => "preference",
            LearningKind::Inference => "inference",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(LearningKind::Fact),
            "preference" => Some(LearningKind::Preference),
            "inference" => Some(LearningKind::Inference),
            _ => None,
        }
    }
}

impl LearningStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LearningStatus::Pending => "pending",
            LearningStatus::Validated => "validated",
            LearningStatus::Rejected => "rejected",
            LearningStatus::Stale => "stale",
            LearningStatus::Promoted => "promoted",
            LearningStatus::Promoting => "promoting",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(LearningStatus::Pending),
            "validated" => Some(LearningStatus::Validated),
            "rejected" => Some(LearningStatus::Rejected),
            "stale" => Some(LearningStatus::Stale),
            "promoted" => Some(LearningStatus::Promoted),
            "promoting" => Some(LearningStatus::Promoting),
            _ => None,
        }
    }
}

impl LearningScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            LearningScope::User => "user",
            LearningScope::Project => "project",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "user" => Some(LearningScope::User),
            "project" => Some(LearningScope::Project),
            _ => None,
        }
    }
}

impl Faithfulness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Faithfulness::Entailment => "entailment",
            Faithfulness::Neutral => "neutral",
            Faithfulness::Contradiction => "contradiction",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "entailment" => Some(Faithfulness::Entailment),
            "neutral" => Some(Faithfulness::Neutral),
            "contradiction" => Some(Faithfulness::Contradiction),
            _ => None,
        }
    }
}
