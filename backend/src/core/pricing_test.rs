use super::*;

/// Counters of the KT-837 execution `93aac172`, read from the Codex journal
/// (`total_token_usage`): 25 209 778 input of which 24 851 584 cached, and
/// 51 617 output. Kronn priced this run at $111.15 by splitting the 25 261 395
/// total 60/40 at 2 $/M in and 8 $/M out.
const KT837_INPUT_INCLUDING_CACHE: u64 = 25_209_778;
const KT837_CACHED: u64 = 24_851_584;
const KT837_OUTPUT: u64 = 51_617;
const KT837_UNCACHED_INPUT: u64 = 358_194;

fn kt837() -> TokenCounters {
    TokenCounters::from_agent_report(
        "Codex",
        KT837_INPUT_INCLUDING_CACHE,
        KT837_OUTPUT,
        Some(KT837_CACHED),
        None,
    )
    .expect("the journal counters are consistent")
}

fn usd(outcome: CostOutcome) -> f64 {
    outcome.usd().expect("expected a known cost")
}

#[test]
fn codex_input_is_split_into_uncached_and_cached() {
    let counters = kt837();
    assert_eq!(counters.input_tokens, KT837_UNCACHED_INPUT);
    assert_eq!(counters.cache_read_tokens, Some(KT837_CACHED));
    assert_eq!(counters.output_tokens, KT837_OUTPUT);
    // The four counters are disjoint: their sum is the total Codex reports.
    assert_eq!(
        counters.traffic_tokens(),
        KT837_INPUT_INCLUDING_CACHE + KT837_OUTPUT
    );
    assert_eq!(counters.traffic_tokens(), 25_261_395);
}

#[test]
fn kt837_counters_no_longer_cost_111_dollars() {
    // The historical figure: the total split 60/40 at gpt-4.1's headline rates.
    let legacy = 25_261_395_f64 * (0.6 * 2.0 + 0.4 * 8.0) / 1_000_000.0;
    assert!((legacy - 111.15).abs() < 0.01);

    let cost = usd(message_cost("Codex", Some("gpt-4.1"), Some(&kt837())));
    // 358 194 x 2 + 24 851 584 x 0.50 + 51 617 x 8, per million.
    let expected = (358_194.0 * 2.0 + 24_851_584.0 * 0.5 + 51_617.0 * 8.0) / 1_000_000.0;
    assert!(
        (cost - expected).abs() < 1e-9,
        "got {cost}, want {expected}"
    );
    assert!((cost - 13.56).abs() < 0.01, "got {cost}");
    assert!(cost < legacy / 8.0, "cache must no longer bill as input");
}

#[test]
fn the_cache_is_billed_at_the_cache_rate_not_the_input_rate() {
    let counters = TokenCounters::from_disjoint_input(0, 0, Some(1_000_000), Some(0));
    let cost = usd(message_cost("Codex", Some("gpt-4.1"), Some(&counters)));
    assert!(
        (cost - 0.5).abs() < 1e-9,
        "1M cached at 0.50 $/M, got {cost}"
    );
}

#[test]
fn a_model_without_a_confirmed_rate_is_unknown_not_borrowed_from_another() {
    assert_eq!(
        message_cost("Codex", Some("gpt-5.7-unlisted"), Some(&kt837())),
        CostOutcome::Unknown(UnknownCost::NoRateForModel)
    );
    // A sibling id must not resolve to the family it merely starts with.
    assert_eq!(
        message_cost("Codex", Some("gpt-4.1-experimental"), Some(&kt837())),
        CostOutcome::Unknown(UnknownCost::NoRateForModel)
    );
}

#[test]
fn a_run_that_recorded_no_model_is_unknown() {
    for model in [None, Some(""), Some("  ")] {
        assert_eq!(
            message_cost("Codex", model, Some(&kt837())),
            CostOutcome::Unknown(UnknownCost::ModelNotReported)
        );
    }
}

#[test]
fn a_bare_token_total_is_never_split_into_a_figure() {
    // No counters at all: the 60/40 guess is gone, for every agent that used to have one.
    for agent in [
        "ClaudeCode",
        "Codex",
        "GeminiCli",
        "Vibe",
        "Kiro",
        "CopilotCli",
    ] {
        let outcome = message_cost(agent, Some("gpt-4.1"), None);
        assert_eq!(
            outcome,
            CostOutcome::Unknown(UnknownCost::NoTokenBreakdown),
            "agent={agent}"
        );
        assert_eq!(outcome.usd(), None, "agent={agent}");
    }
}

#[test]
fn an_agent_whose_cache_accounting_is_unknown_yields_no_counters() {
    for agent in [
        "GeminiCli",
        "Vibe",
        "Kiro",
        "CopilotCli",
        "OpenCode",
        "LiteLlm",
    ] {
        assert!(
            TokenCounters::from_agent_report(agent, 1_000, 500, Some(0), Some(0)).is_none(),
            "agent={agent}"
        );
    }
}

#[test]
fn unreported_cache_counters_make_the_cost_unknown_not_zero_cache() {
    // Codex without `cached_input_tokens`: the input may be mostly cache.
    assert!(TokenCounters::from_agent_report("Codex", 1_000, 10, None, None).is_none());

    let no_read = TokenCounters::from_disjoint_input(1_000, 10, None, Some(0));
    assert_eq!(
        message_cost("ClaudeCode", Some("claude-sonnet-4-5"), Some(&no_read)),
        CostOutcome::Unknown(UnknownCost::CacheReadNotReported)
    );

    let no_write = TokenCounters::from_disjoint_input(1_000, 10, Some(0), None);
    assert_eq!(
        message_cost("ClaudeCode", Some("claude-sonnet-4-5"), Some(&no_write)),
        CostOutcome::Unknown(UnknownCost::CacheWriteNotReported)
    );
}

#[test]
fn a_cached_share_larger_than_the_input_is_rejected_as_inconsistent() {
    assert!(TokenCounters::from_inclusive_input(100, 10, Some(101), None).is_none());
}

#[test]
fn cache_writes_are_priced_only_where_the_model_bills_them() {
    // OpenAI does not bill writes apart from input: an unreported write count is
    // fine, a reported zero too, a reported non-zero one is not priceable.
    let unreported = TokenCounters::from_disjoint_input(1_000_000, 0, Some(0), None);
    assert!((usd(message_cost("Codex", Some("gpt-4.1"), Some(&unreported))) - 2.0).abs() < 1e-9);
    let zero = TokenCounters::from_disjoint_input(1_000_000, 0, Some(0), Some(0));
    assert!((usd(message_cost("Codex", Some("gpt-4.1"), Some(&zero))) - 2.0).abs() < 1e-9);
    let written = TokenCounters::from_disjoint_input(0, 0, Some(0), Some(10));
    assert_eq!(
        message_cost("Codex", Some("gpt-4.1"), Some(&written)),
        CostOutcome::Unknown(UnknownCost::CacheWriteUnpriced)
    );
}

#[test]
fn claude_cost_uses_the_four_counters_and_anthropic_cache_multipliers() {
    // 1M of each: 3 (input) + 0.30 (read) + 3.75 (write) + 15 (output) = 22.05.
    let counters = TokenCounters::from_agent_report(
        "ClaudeCode",
        1_000_000,
        1_000_000,
        Some(1_000_000),
        Some(1_000_000),
    )
    .unwrap();
    // Anthropic reports the input apart from the cache: nothing is subtracted.
    assert_eq!(counters.input_tokens, 1_000_000);
    let cost = usd(message_cost(
        "ClaudeCode",
        Some("claude-sonnet-4-5-20250929"),
        Some(&counters),
    ));
    assert!((cost - 22.05).abs() < 1e-9, "got {cost}");
}

#[test]
fn snapshot_suffixes_resolve_to_their_family_and_nothing_else_does() {
    for model in [
        "gpt-4.1",
        "GPT-4.1",
        "gpt-4.1-2025-04-14",
        "claude-opus-4-1-20250805",
        "claude-3-5-haiku-latest",
    ] {
        assert!(rates_for_model(model).is_some(), "model={model}");
    }
    // The dated form of one family never lands on a neighbouring one.
    assert_eq!(
        rates_for_model("gpt-4.1-mini-2025-04-14"),
        rates_for_model("gpt-4.1-mini")
    );
    assert_ne!(
        rates_for_model("gpt-4.1-mini-2025-04-14"),
        rates_for_model("gpt-4.1")
    );
    for model in [
        "gpt-5.7-unlisted",
        "gpt-4.1-preview",
        "claude-sonnet-4-5[1m]",
        "unknown",
    ] {
        assert!(rates_for_model(model).is_none(), "model={model}");
    }
}

#[test]
fn ollama_is_free_whatever_was_reported_and_litellm_is_not_assumed_free() {
    assert_eq!(message_cost("Ollama", None, None), CostOutcome::Known(0.0));
    assert_eq!(
        message_cost("Ollama", Some("qwen3:32b"), None),
        CostOutcome::Known(0.0)
    );
    // LiteLLM proxies an arbitrary upstream: never 0.0.
    assert_eq!(
        message_cost("LiteLlm", Some("some-cloud-model"), None),
        CostOutcome::Unknown(UnknownCost::NoTokenBreakdown)
    );
    assert_eq!(message_cost("UnknownAgent", None, None).usd(), None);
}

#[test]
fn every_unknown_reason_can_be_stated() {
    for reason in [
        UnknownCost::NoTokenBreakdown,
        UnknownCost::CacheReadNotReported,
        UnknownCost::CacheWriteNotReported,
        UnknownCost::CacheWriteUnpriced,
        UnknownCost::ModelNotReported,
        UnknownCost::NoRateForModel,
    ] {
        assert!(!reason.reason().is_empty());
    }
}

// ── the decision persisted for a finished reply ─────────────────────────

/// What the reply path does with a Codex run's reported usage: resolve the
/// counters for the agent, then price them at the model that served the reply.
fn codex_reply(
    model: Option<&str>,
    tokens_used: u64,
    counters: Option<TokenCounters>,
) -> PricedReply {
    price_reply("Codex", model, tokens_used, None, counters)
}

#[test]
fn a_reply_priced_from_the_real_kt837_counters_is_not_111_dollars() {
    let priced = codex_reply(Some("gpt-4.1"), 25_261_395, Some(kt837()));
    let cost = priced.cost_usd.expect("counters and a rate are both known");
    assert!((cost - 13.56).abs() < 0.01, "got {cost}");
    assert!(
        (cost - 111.15).abs() > 90.0,
        "still the 60/40 figure: {cost}"
    );
    assert_eq!(priced.cost_unknown, None);
    assert_eq!(priced.counters, Some(kt837()));
}

#[test]
fn a_reply_with_only_a_total_is_persisted_unknown_with_its_reason() {
    // The same 25 261 395-token total, but no counters: the old code answered
    // $111.15 here. The answer is now "unknown", and it says why.
    let priced = codex_reply(Some("gpt-4.1"), 25_261_395, None);
    assert_eq!(priced.cost_usd, None);
    assert_eq!(priced.cost_unknown, Some(UnknownCost::NoTokenBreakdown));
}

#[test]
fn a_reply_on_a_model_without_a_rate_is_persisted_unknown_with_its_reason() {
    let priced = codex_reply(Some("gpt-5.7-unlisted"), 25_261_395, Some(kt837()));
    assert_eq!(priced.cost_usd, None);
    assert_eq!(priced.cost_unknown, Some(UnknownCost::NoRateForModel));
    // The counters survive: the split is still worth showing without a price.
    assert_eq!(priced.counters, Some(kt837()));
}

#[test]
fn the_agents_own_reported_cost_wins_and_needs_no_explanation() {
    let priced = price_reply("ClaudeCode", None, 1_000, Some(0.42), None);
    assert_eq!(priced.cost_usd, Some(0.42));
    assert_eq!(priced.cost_unknown, None);
}

#[test]
fn a_reply_that_consumed_nothing_has_no_cost_to_explain() {
    let priced = price_reply("Codex", Some("gpt-4.1"), 0, None, None);
    assert_eq!(priced.cost_usd, None);
    assert_eq!(priced.cost_unknown, None);
}

#[test]
fn local_inference_stays_free_with_only_a_total() {
    let priced = price_reply("Ollama", Some("qwen3:32b"), 12_000, None, None);
    assert_eq!(priced.cost_usd, Some(0.0));
    assert_eq!(priced.cost_unknown, None);
}

#[test]
fn kt837_on_the_model_it_really_ran_on_costs_about_twelve_dollars() {
    // gpt-5.6-sol: 4 $/M input, 0.40 $/M cache read, 20 $/M output.
    let cost = usd(message_cost("Codex", Some("gpt-5.6-sol"), Some(&kt837())));
    assert!((cost - 12.405_749_6).abs() < 1e-6, "got {cost}");
}
