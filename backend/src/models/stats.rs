// Token usage analytics — aggregates rolled up across providers, projects,
// agents, discussions, and workflows. Powers the Stats / Analytics pages.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Cost aggregated across one or more token-cost observations. Never
/// fabricates a price: a dollar amount only lands in `known_usd` when it is
/// either a real observed `cost_usd` (from the provider's own result, may
/// legitimately be 0.0) or a justified per-provider pricing-table estimate.
///
/// See KT-637: substituting 0.0 for a missing cost, or pricing an agent with
/// another provider's table, is a fabrication and must never happen again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostAggregate {
    /// Sum of real observed costs plus justified pricing-table estimates.
    /// 0.0 means "nothing known adds up to zero", not "this is free" —
    /// check `unknown_cost_tokens` before treating this as a complete total.
    pub known_usd: f64,
    /// True when `known_usd` blends at least one pricing-table estimate
    /// rather than only real observed costs. Consumers must label the
    /// figure as including an estimate, not present it as an exact measurement.
    pub has_estimate: bool,
    /// Token count with neither an observed cost nor a pricing-table entry
    /// (e.g. OpenCode, Nvidia, Custom, LiteLLM, or a run with no agent
    /// attribution at all). Excluded from `known_usd`; non-zero means the
    /// total is partial, not complete.
    pub unknown_cost_tokens: u64,
}

impl CostAggregate {
    /// Fold one more (tokens, observed cost, agent) observation in.
    ///
    /// `messages.cost_usd` is not exclusively a real measurement: only
    /// Claude Code's own CLI reports a genuine cost (its `stream-json`
    /// result line). Every other agent's persisted `cost_usd`, when
    /// present, was itself computed from this same pricing table at
    /// message-ingest time (see `discussions/streaming.rs`), so it must
    /// still be flagged as an estimate here — never silently upgraded to
    /// "measured" just because a DB row happens to be non-null.
    pub fn add(&mut self, tokens: u64, cost_db: Option<f64>, agent_type: Option<&str>) {
        let agent = agent_type.unwrap_or("");
        if let Some(recorded) = cost_db {
            self.known_usd += recorded;
            if agent != "ClaudeCode" {
                self.has_estimate = true;
            }
            return;
        }
        match crate::core::pricing::estimate_cost(agent, tokens) {
            Some(estimated) => {
                self.known_usd += estimated;
                self.has_estimate = true;
            }
            None => self.unknown_cost_tokens += tokens,
        }
    }

    /// Combine two aggregates (e.g. rolling per-provider totals into a grand total).
    pub fn merge(mut self, other: &CostAggregate) -> Self {
        self.known_usd += other.known_usd;
        self.has_estimate = self.has_estimate || other.has_estimate;
        self.unknown_cost_tokens += other.unknown_cost_tokens;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsageSummary {
    pub total_tokens: u64,
    pub total_cost: CostAggregate,
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
    pub cost: CostAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderUsage {
    pub provider: String,
    pub tokens_used: u64,
    pub tokens_limit: Option<u64>,
    pub cost: CostAggregate,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectUsage {
    pub project_id: String,
    pub project_name: String,
    pub tokens_used: u64,
    pub cost: CostAggregate,
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
    pub cost: CostAggregate,
    pub anthropic: u64,
    pub openai: u64,
    pub google: u64,
    pub mistral: u64,
    pub amazon: u64,
    pub github: u64,
}

#[cfg(test)]
mod cost_aggregate_tests {
    use super::CostAggregate;

    #[test]
    fn measured_cost_is_known_and_never_flagged_as_estimate() {
        let mut agg = CostAggregate::default();
        agg.add(1000, Some(0.42), Some("ClaudeCode"));
        assert_eq!(agg.known_usd, 0.42);
        assert!(!agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn measured_zero_cost_stays_known_not_unknown() {
        let mut agg = CostAggregate::default();
        agg.add(500, Some(0.0), Some("ClaudeCode"));
        assert_eq!(agg.known_usd, 0.0);
        assert!(!agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn a_persisted_non_claude_cost_is_still_flagged_as_an_estimate() {
        // Only Claude Code's CLI reports a genuine measurement; every other
        // agent's persisted `cost_usd` was itself computed from the pricing
        // table at ingest time and must not be relabeled as "measured".
        let mut agg = CostAggregate::default();
        agg.add(100_000, Some(0.44), Some("Codex"));
        assert_eq!(agg.known_usd, 0.44);
        assert!(agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn missing_cost_with_known_pricing_is_an_estimate() {
        let mut agg = CostAggregate::default();
        agg.add(100_000, None, Some("Codex"));
        assert!(agg.known_usd > 0.0);
        assert!(agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn missing_cost_with_no_pricing_is_unknown_not_zero() {
        for agent in ["OpenCode", "Nvidia", "Custom", "LiteLlm"] {
            let mut agg = CostAggregate::default();
            agg.add(1000, None, Some(agent));
            assert_eq!(agg.known_usd, 0.0, "agent={agent}");
            assert!(!agg.has_estimate, "agent={agent}");
            assert_eq!(agg.unknown_cost_tokens, 1000, "agent={agent}");
        }
    }

    #[test]
    fn no_agent_attribution_is_unknown() {
        let mut agg = CostAggregate::default();
        agg.add(1000, None, None);
        assert_eq!(agg.known_usd, 0.0);
        assert_eq!(agg.unknown_cost_tokens, 1000);
    }

    #[test]
    fn merge_sums_known_cost_and_ors_the_estimate_flag() {
        let mut measured = CostAggregate::default();
        measured.add(1000, Some(1.0), Some("ClaudeCode"));
        let mut estimated = CostAggregate::default();
        estimated.add(2000, None, Some("Codex"));
        let mut unknown = CostAggregate::default();
        unknown.add(3000, None, Some("OpenCode"));

        let total = measured.merge(&estimated).merge(&unknown);
        assert_eq!(total.known_usd, measured.known_usd + estimated.known_usd);
        assert!(total.has_estimate);
        assert_eq!(total.unknown_cost_tokens, 3000);
    }
}
