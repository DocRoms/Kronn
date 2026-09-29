//! Per-model pricing for message cost, computed from the DETAILED token counters.
//!
//! Prices are USD per 1M tokens. A message's cost is only ever derived from the
//! four counters a provider actually reports (uncached input, cache reads, cache
//! writes, output) and the rates of the model that served it. There is no
//! agent-level default rate and no assumed input/output split: KT-894 found a
//! 25.2M-token Codex run priced at $111 because the total was split 60/40 and
//! the cache (98.6% of the volume) was billed at the full input rate. When a
//! counter or a rate is missing the answer is `CostOutcome::Unknown` with the
//! reason, never a figure.
//!
//! Rates updated: 2026-09-29. Sources: OpenAI and Anthropic pricing pages.
//! A model absent from the table is unknown on purpose — add its row once its
//! rate is confirmed rather than borrowing another model's.

/// The counters one message is billed on. Every field is disjoint from the
/// others: `input_tokens` is the input that was NOT served from cache and is NOT
/// a cache write, so the four never overlap and a sum of them is the traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCounters {
    /// Input billed at the full input rate.
    pub input_tokens: u64,
    /// Input served from the prompt cache. `None` means the runtime did not
    /// report it — which is not zero, and makes the cost unknowable.
    pub cache_read_tokens: Option<u64>,
    /// Input written to the prompt cache. `None` means not reported.
    pub cache_write_tokens: Option<u64>,
    pub output_tokens: u64,
}

impl TokenCounters {
    /// Anthropic shape: `input_tokens` already excludes the cache reads and
    /// writes, which are reported apart.
    pub fn from_disjoint_input(
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
    ) -> Self {
        Self {
            input_tokens,
            cache_read_tokens,
            cache_write_tokens,
            output_tokens,
        }
    }

    /// OpenAI shape: `input_tokens` INCLUDES the cached share
    /// (`cached_input_tokens`), so the uncached input is the difference. `None`
    /// when the cached share is missing or larger than the input it is part of:
    /// either way the split cannot be trusted.
    pub fn from_inclusive_input(
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
    ) -> Option<Self> {
        let cached = cached_input_tokens?;
        Some(Self {
            input_tokens: input_tokens.checked_sub(cached)?,
            cache_read_tokens: Some(cached),
            cache_write_tokens,
            output_tokens,
        })
    }

    /// Counters from what an agent's usage report carries, keyed by how THAT
    /// agent accounts its input. `None` for an agent whose cache accounting is
    /// not known here: its input total cannot be split, so it stays unpriced.
    pub fn from_agent_report(
        agent_type: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: Option<u64>,
        cache_write_tokens: Option<u64>,
    ) -> Option<Self> {
        match agent_type {
            "ClaudeCode" => Some(Self::from_disjoint_input(
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            )),
            "Codex" => Self::from_inclusive_input(
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
            ),
            _ => None,
        }
    }

    /// Everything the provider moved: the four counters summed.
    pub fn traffic_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.cache_read_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_tokens.unwrap_or(0))
            .saturating_add(self.output_tokens)
    }
}

/// Why a cost could not be computed. Kept as a closed set so a UI or a log can
/// state the reason instead of showing a bare dash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownCost {
    /// Only a token total was reported (or the agent's accounting is unknown).
    NoTokenBreakdown,
    /// The counters did not say whether input was served from cache.
    CacheReadNotReported,
    /// The counters did not say how much input was written to the cache, and
    /// this model bills cache writes apart from input.
    CacheWriteNotReported,
    /// Cache writes were reported, but this model has no separate write rate to
    /// price them with.
    CacheWriteUnpriced,
    /// The run did not record which model served it.
    ModelNotReported,
    /// No confirmed rate for the served model.
    NoRateForModel,
}

impl UnknownCost {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NoTokenBreakdown => "only a token total was reported, not the input/output split",
            Self::CacheReadNotReported => "cache reads were not reported",
            Self::CacheWriteNotReported => "cache writes were not reported",
            Self::CacheWriteUnpriced => {
                "cache writes were reported but this model has no write rate"
            }
            Self::ModelNotReported => "the serving model was not recorded",
            Self::NoRateForModel => "no confirmed rate for the serving model",
        }
    }
}

/// A computed cost, or the reason there is none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CostOutcome {
    Known(f64),
    Unknown(UnknownCost),
}

impl CostOutcome {
    /// The figure to persist: `None` is "unknown", which is never `0.0`.
    pub fn usd(self) -> Option<f64> {
        match self {
            Self::Known(usd) => Some(usd),
            Self::Unknown(_) => None,
        }
    }
}

/// Rates of one model, per 1M tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ModelRates {
    input_per_m: f64,
    cache_read_per_m: f64,
    /// `None` when the provider does not bill cache writes apart from input.
    cache_write_per_m: Option<f64>,
    output_per_m: f64,
}

/// OpenAI bills cached input at a model-specific fraction of input and does not
/// charge for writing the cache.
const fn openai(input_per_m: f64, cache_read_per_m: f64, output_per_m: f64) -> ModelRates {
    ModelRates {
        input_per_m,
        cache_read_per_m,
        cache_write_per_m: None,
        output_per_m,
    }
}

/// Anthropic's published cache multipliers: reads at 0.1x input, 5-minute
/// writes at 1.25x input.
const fn anthropic(input_per_m: f64, output_per_m: f64) -> ModelRates {
    ModelRates {
        input_per_m,
        cache_read_per_m: input_per_m * 0.1,
        cache_write_per_m: Some(input_per_m * 1.25),
        output_per_m,
    }
}

/// Model id (dated and `-latest` suffixes removed) -> rates.
const MODEL_RATES: &[(&str, ModelRates)] = &[
    // OpenAI
    ("gpt-4.1", openai(2.0, 0.5, 8.0)),
    ("gpt-4.1-mini", openai(0.4, 0.1, 1.6)),
    ("gpt-4.1-nano", openai(0.1, 0.025, 0.4)),
    ("gpt-4o", openai(2.5, 1.25, 10.0)),
    ("gpt-4o-mini", openai(0.15, 0.075, 0.6)),
    ("gpt-5", openai(1.25, 0.125, 10.0)),
    ("gpt-5-codex", openai(1.25, 0.125, 10.0)),
    ("gpt-5-mini", openai(0.25, 0.025, 2.0)),
    // Anthropic
    ("claude-3-5-haiku", anthropic(0.8, 4.0)),
    ("claude-haiku-4-5", anthropic(1.0, 5.0)),
    ("claude-3-7-sonnet", anthropic(3.0, 15.0)),
    ("claude-sonnet-4", anthropic(3.0, 15.0)),
    ("claude-sonnet-4-5", anthropic(3.0, 15.0)),
    ("claude-opus-4", anthropic(15.0, 75.0)),
    ("claude-opus-4-1", anthropic(15.0, 75.0)),
    ("claude-opus-4-5", anthropic(5.0, 25.0)),
];

/// Drop a trailing `-latest`, `-YYYY-MM-DD` or `-YYYYMMDD` so a snapshot id
/// resolves to its family. Anything else is left alone: an unrecognised suffix
/// must not fall through to a neighbouring model's rate.
fn strip_snapshot_suffix(model: &str) -> &str {
    if let Some(family) = model.strip_suffix("-latest") {
        return family;
    }
    let bytes = model.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    let len = bytes.len();
    // -YYYYMMDD
    if len > 9 && bytes[len - 9] == b'-' && digits(len - 8..len) {
        return &model[..len - 9];
    }
    // -YYYY-MM-DD
    if len > 11
        && bytes[len - 11] == b'-'
        && bytes[len - 6] == b'-'
        && bytes[len - 3] == b'-'
        && digits(len - 10..len - 6)
        && digits(len - 5..len - 3)
        && digits(len - 2..len)
    {
        return &model[..len - 11];
    }
    model
}

fn rates_for_model(model: &str) -> Option<ModelRates> {
    let normalized = model.trim().to_ascii_lowercase();
    let family = strip_snapshot_suffix(&normalized);
    MODEL_RATES
        .iter()
        .find(|(id, _)| *id == family)
        .map(|(_, rates)| *rates)
}

/// Cost of one message.
///
/// `model` is the model that actually served it. `counters` is `None` when the
/// runtime only reported a token total.
pub fn message_cost(
    agent_type: &str,
    model: Option<&str>,
    counters: Option<&TokenCounters>,
) -> CostOutcome {
    // Local inference costs nothing whatever the token split, so it needs
    // neither counters nor a rate. LiteLLM, by contrast, proxies an arbitrary
    // upstream: it falls through and is unknown unless a counter AND a rate say
    // otherwise.
    if agent_type == "Ollama" {
        return CostOutcome::Known(0.0);
    }
    let Some(counters) = counters else {
        return CostOutcome::Unknown(UnknownCost::NoTokenBreakdown);
    };
    let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) else {
        return CostOutcome::Unknown(UnknownCost::ModelNotReported);
    };
    let Some(rates) = rates_for_model(model) else {
        return CostOutcome::Unknown(UnknownCost::NoRateForModel);
    };
    let Some(cache_read) = counters.cache_read_tokens else {
        return CostOutcome::Unknown(UnknownCost::CacheReadNotReported);
    };
    let cache_write_usd = match (counters.cache_write_tokens, rates.cache_write_per_m) {
        (Some(tokens), Some(rate)) => tokens as f64 * rate,
        (None, Some(_)) => return CostOutcome::Unknown(UnknownCost::CacheWriteNotReported),
        (None | Some(0), None) => 0.0,
        (Some(_), None) => return CostOutcome::Unknown(UnknownCost::CacheWriteUnpriced),
    };
    let usd = (counters.input_tokens as f64 * rates.input_per_m
        + cache_read as f64 * rates.cache_read_per_m
        + cache_write_usd
        + counters.output_tokens as f64 * rates.output_per_m)
        / 1_000_000.0;
    CostOutcome::Known(usd)
}

/// The pricing decision for one finished reply: what to persist as its cost, the
/// counters worth keeping, and why the cost is missing when it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricedReply {
    /// `None` is unknown — never `0.0`.
    pub cost_usd: Option<f64>,
    pub counters: Option<TokenCounters>,
    /// Set only when the reply consumed tokens and still has no cost.
    pub cost_unknown: Option<UnknownCost>,
}

/// Price a finished reply. The agent's own reported cost wins when it gives one;
/// otherwise the cost is computed from `counters` at the rates of `model`, the
/// model that served the reply. A reply that consumed nothing has no cost to
/// explain.
pub fn price_reply(
    agent_type: &str,
    model: Option<&str>,
    tokens_used: u64,
    agent_reported_cost: Option<f64>,
    counters: Option<TokenCounters>,
) -> PricedReply {
    let outcome = message_cost(agent_type, model, counters.as_ref());
    let cost_usd =
        agent_reported_cost.or_else(|| if tokens_used > 0 { outcome.usd() } else { None });
    let cost_unknown = match outcome {
        CostOutcome::Unknown(reason) if cost_usd.is_none() && tokens_used > 0 => Some(reason),
        _ => None,
    };
    PricedReply {
        cost_usd,
        counters,
        cost_unknown,
    }
}

#[cfg(test)]
#[path = "pricing_test.rs"]
mod pricing_test;
