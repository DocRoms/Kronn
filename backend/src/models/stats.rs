// Token usage analytics — aggregates rolled up across providers, projects,
// agents, discussions, and workflows. Powers the Stats / Analytics pages.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Cost aggregated across one or more token-cost observations. Never
/// fabricates a price and never collapses distinct provenances into one
/// number: a persisted DB amount, a freshly-computed pricing estimate, and a
/// true absence of data are three different things and stay in three
/// different fields.
///
/// See KT-637: substituting 0.0 for a missing cost, pricing an agent with
/// another provider's table, or asserting a persisted `cost_usd` is a real
/// measurement (it is not, for ANY agent — see `add`) are all fabrications
/// and must never happen again.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CostAggregate {
    /// Sum of `messages.cost_usd` (or `workflow_runs`/equivalent) values
    /// already persisted for tokens in this group. Provenance is NOT
    /// guaranteed to be a real measurement — see `add`. 0.0 with
    /// `has_recorded == true` means "recorded as free", which is different
    /// from `has_recorded == false` ("nothing recorded at all").
    pub recorded_usd: f64,
    /// True once at least one token in this group had a persisted, non-null
    /// cost folded into `recorded_usd`.
    pub has_recorded: bool,
    /// Sum of pricing-table estimates computed here (never persisted), for
    /// tokens that had no recorded cost at all. Always a genuine,
    /// freshly-computed estimate — never a relabeled recorded amount.
    pub estimated_usd: f64,
    /// True once at least one token's cost came from `estimated_usd`.
    pub has_estimate: bool,
    /// Token count with neither a recorded cost nor a pricing-table entry
    /// (e.g. OpenCode, Nvidia, Custom, LiteLLM, or a run with no agent
    /// attribution at all). Non-zero means `recorded_usd + estimated_usd`
    /// is a partial total, not a complete one.
    pub unknown_cost_tokens: u64,
}

impl CostAggregate {
    /// Total of every dollar we have some basis for. Never present this as
    /// "the true cost" on its own — check `unknown_cost_tokens` for
    /// completeness and `has_recorded`/`has_estimate` for provenance.
    pub fn known_usd(&self) -> f64 {
        self.recorded_usd + self.estimated_usd
    }

    /// Fold one more (agent, tokens) group in, already split by the caller
    /// into a recorded-cost sum for the group's rows that had a persisted
    /// cost, and a token count for the rows that had none. `SUM(cost_usd)`
    /// in SQL silently drops NULL rows, so that split MUST happen in the
    /// query itself (a per-group `SUM(tokens_used)` alongside a plain
    /// `SUM(cost_usd)` is not enough — see KT-637 review) or every unpriced
    /// row in a partially-priced group gets misreported as priced.
    ///
    /// `messages.cost_usd` is not exclusively a real measurement for any
    /// agent, including ClaudeCode: the ingest path
    /// (`discussions/streaming.rs`) falls back to this same pricing table
    /// whenever the agent's own reported cost isn't available, and the DB
    /// stores no column distinguishing the two cases. So a non-null
    /// `cost_usd` is folded into `recorded_usd` with unguaranteed
    /// provenance — never asserted "measured" — regardless of agent_type.
    pub fn add(
        &mut self,
        recorded_cost_sum: Option<f64>,
        unrecorded_tokens: u64,
        agent_type: Option<&str>,
    ) {
        if let Some(recorded) = recorded_cost_sum {
            self.recorded_usd += recorded;
            self.has_recorded = true;
        }
        if unrecorded_tokens > 0 {
            let agent = agent_type.unwrap_or("");
            match crate::core::pricing::estimate_cost(agent, unrecorded_tokens) {
                Some(estimated) => {
                    self.estimated_usd += estimated;
                    self.has_estimate = true;
                }
                None => self.unknown_cost_tokens += unrecorded_tokens,
            }
        }
    }

    /// Combine two aggregates (e.g. rolling per-provider totals into a grand total).
    pub fn merge(mut self, other: &CostAggregate) -> Self {
        self.recorded_usd += other.recorded_usd;
        self.has_recorded = self.has_recorded || other.has_recorded;
        self.estimated_usd += other.estimated_usd;
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
    fn recorded_cost_is_known_but_never_asserted_as_a_measurement() {
        // A persisted cost (any agent, including ClaudeCode) lands in
        // `recorded_usd` — its provenance is unguaranteed, so it must never
        // be asserted as either "definitely measured" or "definitely an
        // estimate" by this call alone.
        let mut agg = CostAggregate::default();
        agg.add(Some(0.42), 0, Some("ClaudeCode"));
        assert_eq!(agg.recorded_usd, 0.42);
        assert!(agg.has_recorded);
        assert_eq!(agg.known_usd(), 0.42);
        assert!(!agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn recorded_zero_cost_stays_recorded_not_unknown() {
        let mut agg = CostAggregate::default();
        agg.add(Some(0.0), 0, Some("ClaudeCode"));
        assert_eq!(agg.recorded_usd, 0.0);
        assert!(agg.has_recorded);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn a_recorded_cost_from_any_agent_is_not_auto_flagged_as_an_estimate() {
        // The ingest path falls back to the pricing table for EVERY agent,
        // including ClaudeCode, whenever the agent's own reported cost is
        // unavailable (see `add`'s doc). The DB stores no column to tell a
        // real measurement apart from that fallback, for any agent — so a
        // non-Claude persisted cost must not be branded "definitely an
        // estimate" either; it is recorded, provenance unguaranteed, same as
        // ClaudeCode's.
        let mut agg = CostAggregate::default();
        agg.add(Some(0.44), 0, Some("Codex"));
        assert_eq!(agg.recorded_usd, 0.44);
        assert!(agg.has_recorded);
        assert!(!agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn missing_cost_with_known_pricing_is_an_estimate() {
        let mut agg = CostAggregate::default();
        agg.add(None, 100_000, Some("Codex"));
        assert!(agg.estimated_usd > 0.0);
        assert_eq!(agg.known_usd(), agg.estimated_usd);
        assert!(agg.has_estimate);
        assert!(!agg.has_recorded);
        assert_eq!(agg.unknown_cost_tokens, 0);
    }

    #[test]
    fn missing_cost_with_no_pricing_is_unknown_not_zero() {
        for agent in ["OpenCode", "Nvidia", "Custom", "LiteLlm"] {
            let mut agg = CostAggregate::default();
            agg.add(None, 1000, Some(agent));
            assert_eq!(agg.known_usd(), 0.0, "agent={agent}");
            assert!(!agg.has_estimate, "agent={agent}");
            assert!(!agg.has_recorded, "agent={agent}");
            assert_eq!(agg.unknown_cost_tokens, 1000, "agent={agent}");
        }
    }

    #[test]
    fn no_agent_attribution_is_unknown() {
        let mut agg = CostAggregate::default();
        agg.add(None, 1000, None);
        assert_eq!(agg.known_usd(), 0.0);
        assert_eq!(agg.unknown_cost_tokens, 1000);
    }

    #[test]
    fn a_group_mixing_recorded_and_unrecorded_rows_keeps_both_shares_honest() {
        // Same agent, same group: some rows have a persisted cost, some
        // don't. The recorded sum must only ever be claimed for the tokens
        // that actually had one — the caller is responsible for excluding
        // NULL-cost tokens from `recorded_cost_sum` and instead counting
        // them into `unrecorded_tokens` (KT-637 review: `SUM(cost_usd)`
        // silently drops NULLs, so this split cannot happen inside `add`).
        let mut agg = CostAggregate::default();
        agg.add(Some(1.0), 200, Some("OpenCode"));
        assert_eq!(agg.recorded_usd, 1.0);
        assert!(agg.has_recorded);
        // OpenCode has no pricing-table entry, so the 200 unrecorded tokens
        // are unknown, not folded into the 1.0 that only covers other rows.
        assert!(!agg.has_estimate);
        assert_eq!(agg.unknown_cost_tokens, 200);
        assert_eq!(agg.known_usd(), 1.0);
    }

    #[test]
    fn merge_sums_each_bucket_and_ors_the_provenance_flags() {
        let mut recorded = CostAggregate::default();
        recorded.add(Some(1.0), 0, Some("ClaudeCode"));
        let mut estimated = CostAggregate::default();
        estimated.add(None, 2000, Some("Codex"));
        let mut unknown = CostAggregate::default();
        unknown.add(None, 3000, Some("OpenCode"));

        let total = recorded.merge(&estimated).merge(&unknown);
        assert_eq!(total.recorded_usd, recorded.recorded_usd);
        assert_eq!(total.estimated_usd, estimated.estimated_usd);
        assert_eq!(
            total.known_usd(),
            recorded.known_usd() + estimated.known_usd()
        );
        assert!(total.has_recorded);
        assert!(total.has_estimate);
        assert_eq!(total.unknown_cost_tokens, 3000);
    }
}
