//! Static pricing table for token cost estimation.
//! Prices are per 1M tokens (input/output) in USD.
//! Updated: 2026-03-31. Source: provider pricing pages.

/// Price per 1M tokens (input, output) in USD.
struct ModelPrice {
    input_per_m: f64,
    output_per_m: f64,
}

/// Estimate cost in USD from token count and agent type string.
/// Returns None if pricing is unknown for this agent.
pub fn estimate_cost(agent_type: &str, tokens_used: u64) -> Option<f64> {
    // Most agents report total tokens (input + output combined).
    // We estimate a 60/40 split (input/output) as a reasonable default.
    let price = match agent_type {
        // Anthropic — Claude Sonnet 4 (most common via Claude Code)
        "ClaudeCode" => ModelPrice { input_per_m: 3.0, output_per_m: 15.0 },
        // OpenAI — GPT-4.1 (Codex default)
        "Codex" => ModelPrice { input_per_m: 2.0, output_per_m: 8.0 },
        // Google — Gemini 2.5 Pro
        "GeminiCli" => ModelPrice { input_per_m: 1.25, output_per_m: 10.0 },
        // Mistral — Mistral Large
        "Vibe" => ModelPrice { input_per_m: 2.0, output_per_m: 6.0 },
        // Kiro — uses Bedrock (Claude), credits already converted
        "Kiro" => ModelPrice { input_per_m: 3.0, output_per_m: 15.0 },
        // GitHub Copilot — uses GPT-4o by default
        "CopilotCli" => ModelPrice { input_per_m: 2.5, output_per_m: 10.0 },
        _ => return None,
    };

    // Assume 60% input, 40% output for combined token counts
    let input_tokens = (tokens_used as f64) * 0.6;
    let output_tokens = (tokens_used as f64) * 0.4;
    let cost = (input_tokens * price.input_per_m + output_tokens * price.output_per_m) / 1_000_000.0;
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_cost_estimation() {
        // 100K tokens → ~60K input + 40K output
        // (60K × 3.0 + 40K × 15.0) / 1M = (180K + 600K) / 1M = 0.78
        let cost = estimate_cost("ClaudeCode", 100_000).unwrap();
        assert!((cost - 0.78).abs() < 0.01, "Expected ~$0.78, got ${:.4}", cost);
    }

    #[test]
    fn codex_cost_estimation() {
        let cost = estimate_cost("Codex", 100_000).unwrap();
        assert!(cost > 0.0 && cost < 1.0);
    }

    #[test]
    fn unknown_agent_returns_none() {
        assert!(estimate_cost("UnknownAgent", 100_000).is_none());
    }

    #[test]
    fn zero_tokens_returns_zero() {
        let cost = estimate_cost("ClaudeCode", 0).unwrap();
        assert_eq!(cost, 0.0);
    }

    #[test]
    fn gemini_cost_estimation() {
        let cost = estimate_cost("GeminiCli", 100_000).unwrap();
        assert!(cost > 0.0 && cost < 1.0, "GeminiCli 100K tokens should cost < $1, got ${:.4}", cost);
    }

    #[test]
    fn vibe_cost_estimation() {
        let cost = estimate_cost("Vibe", 100_000).unwrap();
        assert!(cost > 0.0 && cost < 1.0, "Vibe 100K tokens should cost < $1, got ${:.4}", cost);
    }

    #[test]
    fn kiro_cost_estimation() {
        let cost = estimate_cost("Kiro", 100_000).unwrap();
        assert!(cost > 0.0 && cost < 1.0, "Kiro 100K tokens should cost < $1, got ${:.4}", cost);
    }

    #[test]
    fn copilot_cost_estimation() {
        let cost = estimate_cost("CopilotCli", 100_000).unwrap();
        assert!(cost > 0.0 && cost < 1.0, "CopilotCli 100K tokens should cost < $1, got ${:.4}", cost);
    }

    #[test]
    fn all_known_agents_have_pricing() {
        for agent in &["ClaudeCode", "Codex", "GeminiCli", "Vibe", "Kiro", "CopilotCli"] {
            assert!(estimate_cost(agent, 1000).is_some(), "Missing pricing for {}", agent);
        }
    }
}
