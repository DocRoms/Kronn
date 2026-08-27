use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AgentType, ModelTier};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ComparePromptCompatibility {
    Identical,
    Different,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum CompareImprovementAvailability {
    Available,
    DifferentPrompts,
    MissingPrompt,
    NoSharedQuickPrompt,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchCompareAiEvaluation {
    pub score: u8,
    pub confidence: f64,
    pub positives: Vec<String>,
    pub negatives: Vec<String>,
    pub contract_violations: Vec<String>,
    pub judge_run_id: String,
    pub judge_agent: AgentType,
    pub judge_tier: ModelTier,
    pub judge_model: Option<String>,
    pub judge_duration_ms: Option<u64>,
    pub judge_tokens_used: Option<u64>,
    pub rubric_version: String,
    pub judged_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchCompareEvaluation {
    pub discussion_id: String,
    pub manual_score: Option<u8>,
    pub manual_updated_at: Option<DateTime<Utc>>,
    pub ai: Option<BatchCompareAiEvaluation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum BatchComparePromptImpact {
    All,
    Some,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchComparePromptFinding {
    pub text: String,
    pub affects: BatchComparePromptImpact,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchComparePromptReview {
    pub worth_improving: bool,
    pub strengths: Vec<String>,
    pub weaknesses: Vec<BatchComparePromptFinding>,
    pub recommendations: Vec<BatchComparePromptFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchCompareJudgeRun {
    pub id: String,
    pub status: String,
    pub judge_agent: AgentType,
    pub judge_tier: ModelTier,
    /// True when the selected judge agent also produced one of the candidate
    /// answers. The verdict remains usable but the UI must expose the bias.
    pub self_evaluation: bool,
    pub judge_model: Option<String>,
    pub judge_discussion_id: Option<String>,
    pub rubric_version: String,
    pub prompt_review: Option<BatchComparePromptReview>,
    pub error: Option<String>,
    pub tokens_used: Option<u64>,
    pub duration_ms: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchCompareDetails {
    pub run_id: String,
    pub prompt_compatibility: ComparePromptCompatibility,
    pub improvement_availability: CompareImprovementAvailability,
    pub evaluations: Vec<BatchCompareEvaluation>,
    pub latest_judge_run: Option<BatchCompareJudgeRun>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreateAdHocCompareRequest {
    pub discussion_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CreateAdHocCompareResponse {
    pub run_id: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpdateBatchCompareManualScoreRequest {
    /// `None` clears the human rating; otherwise the accepted range is 1..=5.
    pub score: Option<u8>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct StartBatchCompareJudgeRequest {
    pub agent: AgentType,
    #[serde(default)]
    pub tier: ModelTier,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StartBatchCompareJudgeResponse {
    pub judge_run_id: String,
    pub judge_discussion_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct StartBatchCompareImprovementRequest {
    pub agent: AgentType,
    #[serde(default = "reasoning_tier")]
    pub tier: ModelTier,
}

fn reasoning_tier() -> ModelTier {
    ModelTier::Reasoning
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StartBatchCompareImprovementResponse {
    pub discussion_id: String,
}
