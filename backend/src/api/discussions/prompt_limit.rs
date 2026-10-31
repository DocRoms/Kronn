//! KT-1529 — how large a discussion prompt may be.
//!
//! The limit follows the target model's context window, with headroom for the
//! system prompt, the history and the answer, instead of one fixed number for
//! every model. A server-wide hard ceiling (`server.max_prompt_bytes`) stays as
//! a safety net so a single request can never be unbounded.

use crate::agents::runner::effective_model_flag;
use crate::models::{AgentType, MessageTarget, ModelTier, ModelTiersConfig};
use std::collections::HashMap;

/// Floor for a configured ceiling, so a `0` in config.toml cannot refuse every
/// message.
const MIN_MAX_PROMPT_BYTES: usize = 10_000;

/// Bytes of prompt a model can take per token of its window once the headroom
/// for the system prompt, the history and the answer is set aside. English text
/// runs near 4 bytes per token and code nearer 3; keeping a quarter of the
/// window free lands on 3 for both.
const USABLE_BYTES_PER_WINDOW_TOKEN: u64 = 3;

/// Window assumed for a model we cannot identify, per runtime. Deliberately on
/// the cautious side: refusing too early is recoverable (pick a bigger model),
/// a prompt the model silently truncates is not.
fn agent_default_window_tokens(agent: &AgentType) -> u64 {
    match agent {
        AgentType::ClaudeCode | AgentType::Kiro => 200_000,
        AgentType::GeminiCli => 1_000_000,
        AgentType::Codex => 200_000,
        AgentType::OpenCode
        | AgentType::CopilotCli
        | AgentType::Vibe
        | AgentType::LiteLlm
        | AgentType::Nvidia => 128_000,
        // Local models run with whatever `num_ctx` the machine affords.
        AgentType::Ollama | AgentType::Custom => 32_000,
    }
}

/// Context window (tokens) of `model` when its family is recognisable.
///
/// The model catalogue carries no window metadata, so the families below are
/// the source. The name is matched as a *prefix* (after any `provider/` or
/// `anthropic.` routing prefix), never as a substring, so a community
/// fine-tune such as `llama3-claude-distill` is not mistaken for Claude. A
/// local runtime (`Ollama`, `Custom`) names models freely and its real window
/// is the `num_ctx` the user pinned, so no family is guessed for those.
fn model_window_tokens(agent: &AgentType, model: &str) -> Option<u64> {
    if matches!(agent, AgentType::Ollama | AgentType::Custom) {
        return None;
    }
    let lowered = model.trim().to_ascii_lowercase();
    // `openai/gpt-4.1` (LiteLLM, OpenCode) and `bedrock/anthropic.claude-…`.
    let name = lowered.rsplit('/').next().unwrap_or(&lowered);
    let name = name.rsplit("anthropic.").next().unwrap_or(name);
    let starts_with_any = |families: &[&str]| families.iter().any(|f| name.starts_with(f));
    if starts_with_any(&["gemini", "gpt-4.1"]) {
        Some(1_000_000)
    } else if starts_with_any(&["claude", "opus", "sonnet", "haiku", "gpt-5"])
        || is_openai_reasoning_model(name)
    {
        Some(200_000)
    } else if starts_with_any(&["gpt-4o", "gpt-4-turbo", "deepseek"]) {
        Some(128_000)
    } else {
        None
    }
}

/// OpenAI reasoning series: `o1`, `o3`, `o4-mini`… but not an unrelated name
/// that merely starts with the letter `o`.
fn is_openai_reasoning_model(name: &str) -> bool {
    ["o1", "o3", "o4"].iter().any(|series| {
        name.strip_prefix(series)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
    })
}

/// The size limit that applies to one target, and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PromptLimit {
    pub bytes: usize,
    /// Model (or, failing that, runtime) the limit was derived from.
    pub model: String,
    pub window_tokens: u64,
    /// True when the hard ceiling, not the model window, is what binds.
    pub ceiling_bound: bool,
}

/// Derive the limit for `agent` running `model` (explicit override, else the
/// tier's model). `ollama_overrides` carries the per-model `num_ctx` the user
/// pinned, which is the real window of a local model.
pub(crate) fn limit_for(
    agent: &AgentType,
    model: Option<&str>,
    ollama_overrides: &HashMap<String, u64>,
    hard_ceiling: usize,
) -> PromptLimit {
    let model_name = model.map(str::trim).filter(|m| !m.is_empty());
    let pinned = model_name
        .filter(|_| matches!(agent, AgentType::Ollama))
        .and_then(|m| ollama_overrides.get(m).copied());
    let window_tokens = pinned
        .or_else(|| model_name.and_then(|m| model_window_tokens(agent, m)))
        .unwrap_or_else(|| agent_default_window_tokens(agent));
    let from_window = window_tokens.saturating_mul(USABLE_BYTES_PER_WINDOW_TOKEN);
    let from_window = usize::try_from(from_window).unwrap_or(usize::MAX);
    let ceiling_bound = from_window > hard_ceiling;
    PromptLimit {
        bytes: from_window.min(hard_ceiling),
        model: model_name
            .map(str::to_owned)
            .unwrap_or_else(|| crate::api::disc_helpers::agent_display_name(agent)),
        window_tokens,
        ceiling_bound,
    }
}

/// `Ok` when `len` fits, else the refusal: the limit, the model it comes from
/// and what to do about it. `subject` is the leading noun ("Prompt"/"Message").
pub(crate) fn check(len: usize, limit: &PromptLimit, subject: &str) -> Result<(), String> {
    if len <= limit.bytes {
        return Ok(());
    }
    if limit.ceiling_bound {
        return Err(format!(
            "{subject} too long ({len} bytes): the server-wide ceiling is {} bytes \
             (`server.max_prompt_bytes`, set in Kronn's config.toml) and applies to every model, \
             including {} ({} tokens of context). Attach the content as a file for the agent to \
             read in parts, or raise the ceiling.",
            limit.bytes, limit.model, limit.window_tokens,
        ));
    }
    Err(format!(
        "{subject} too long ({len} bytes): the limit for {} is {} bytes (a ~{}-token context \
         window, minus room for the system prompt, history and answer). Attach the content as a \
         file for the agent to read in parts, or pick a model with a larger context window.",
        limit.model, limit.bytes, limit.window_tokens,
    ))
}

/// Tightest limit across `targets`, each resolved with its own tier.
/// `fallback_agent` stands in when no explicit target is named (the
/// discussion's own agent).
///
/// `model_override` is the discussion's own model pin, so it belongs to the
/// discussion's agent only: the dispatcher drops it for a per-target tier, and
/// a different agent mentioned in the same message runs its own tier model, not
/// the primary agent's.
pub(crate) fn tightest_limit(
    targets: &[MessageTarget],
    fallback_agent: &AgentType,
    discussion_tier: ModelTier,
    model_override: Option<&str>,
    tiers: &ModelTiersConfig,
    ollama_overrides: &HashMap<String, u64>,
    hard_ceiling: usize,
) -> PromptLimit {
    let resolve = |agent: &AgentType, tier: ModelTier, pinned: Option<&str>| {
        let model = effective_model_flag(pinned, agent, tier, Some(tiers));
        limit_for(agent, model.as_deref(), ollama_overrides, hard_ceiling)
    };
    let mut limits: Vec<PromptLimit> = targets
        .iter()
        .map(|t| {
            let pinned =
                model_override.filter(|_| t.agent_type == *fallback_agent && t.tier.is_none());
            resolve(&t.agent_type, t.tier.unwrap_or(discussion_tier), pinned)
        })
        .collect();
    if limits.is_empty() {
        limits.push(resolve(fallback_agent, discussion_tier, model_override));
    }
    limits
        .into_iter()
        .min_by_key(|l| l.bytes)
        .expect("at least one limit")
}

/// Resolve the limit for a discussion's targets from live config.
pub(crate) async fn resolve_for_targets(
    state: &crate::AppState,
    targets: &[MessageTarget],
    fallback_agent: &AgentType,
    discussion_tier: ModelTier,
    model_override: Option<&str>,
) -> PromptLimit {
    let config = state.config.read().await;
    tightest_limit(
        targets,
        fallback_agent,
        discussion_tier,
        model_override,
        &config.agents.model_tiers,
        &config.server.ollama_context_overrides,
        config.server.max_prompt_bytes.max(MIN_MAX_PROMPT_BYTES),
    )
}

/// Check `len` against the discussion's responders: the tightest window among
/// `targets` (or the discussion's own agent when none is named). A room with no
/// native agent and no explicit target is human-only, so only the hard ceiling
/// applies. An unknown discussion passes: the caller reports it as missing.
pub(crate) async fn enforce_for_discussion(
    state: &crate::AppState,
    discussion_id: &str,
    targets: &[MessageTarget],
    len: usize,
    subject: &str,
) -> Result<(), String> {
    let did = discussion_id.to_string();
    let loaded = state
        .db
        .with_conn(move |conn| {
            let disc = crate::db::discussions::get_discussion(conn, &did)?;
            let no_agent = crate::db::discussions::disc_is_no_agent(conn, &did)?;
            Ok(disc.map(|d| (d.agent, d.tier, d.model, no_agent)))
        })
        .await
        .ok()
        .flatten();
    let Some((agent, tier, model, no_agent)) = loaded else {
        return check_hard_ceiling(len, hard_ceiling(state).await, subject);
    };
    if no_agent && targets.is_empty() {
        return check_hard_ceiling(len, hard_ceiling(state).await, subject);
    }
    let limit = resolve_for_targets(state, targets, &agent, tier, model.as_deref()).await;
    check(len, &limit, subject)
}

/// The configured hard ceiling alone, for entry points that have not resolved
/// a target yet (a cheap early refusal before any DB work).
pub(crate) async fn hard_ceiling(state: &crate::AppState) -> usize {
    state
        .config
        .read()
        .await
        .server
        .max_prompt_bytes
        .max(MIN_MAX_PROMPT_BYTES)
}

/// The hard ceiling as saved in config.toml, for a caller that holds no
/// `AppState` (a workflow step runs below the HTTP layer).
pub(crate) fn saved_hard_ceiling() -> usize {
    crate::core::config::saved_max_prompt_bytes()
        .unwrap_or(crate::models::setup::DEFAULT_MAX_PROMPT_BYTES)
        .max(MIN_MAX_PROMPT_BYTES)
}

/// Refusal for `len` bytes against the hard ceiling only, in the same words
/// as the model-derived one.
pub(crate) fn check_hard_ceiling(len: usize, ceiling: usize, subject: &str) -> Result<(), String> {
    if len <= ceiling {
        return Ok(());
    }
    Err(format!(
        "{subject} too long ({len} bytes): the server-wide ceiling is {ceiling} bytes \
         (`server.max_prompt_bytes`, set in Kronn's config.toml) and applies to every model. \
         Attach the content as a file for the agent to read in parts, or raise the ceiling."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::models::setup::DEFAULT_MAX_PROMPT_BYTES;

    const CEILING: usize = DEFAULT_MAX_PROMPT_BYTES;

    fn none() -> HashMap<String, u64> {
        HashMap::new()
    }

    #[test]
    fn a_500_kb_prompt_fits_a_large_context_model() {
        let gemini = limit_for(
            &AgentType::GeminiCli,
            Some("gemini-2.5-pro"),
            &none(),
            CEILING,
        );
        assert!(check(500_000, &gemini, "Prompt").is_ok());
        let claude = limit_for(&AgentType::ClaudeCode, None, &none(), CEILING);
        assert!(check(500_000, &claude, "Prompt").is_ok());
    }

    #[test]
    fn a_500_kb_prompt_is_refused_by_a_small_context_model_with_the_fix() {
        let small = limit_for(&AgentType::Ollama, Some("llama3:8b"), &none(), CEILING);
        let refusal = check(500_000, &small, "Prompt").unwrap_err();
        assert!(
            refusal.starts_with("Prompt too long (500000 bytes)"),
            "{refusal}"
        );
        assert!(refusal.contains("llama3:8b"), "names the model: {refusal}");
        assert!(
            refusal.contains(&small.bytes.to_string()),
            "names the limit: {refusal}"
        );
        assert!(
            refusal.contains("Attach the content as a file"),
            "{refusal}"
        );
        assert!(refusal.contains("larger context window"), "{refusal}");
    }

    #[test]
    fn the_limit_leaves_headroom_below_the_raw_window() {
        let l = limit_for(&AgentType::ClaudeCode, None, &none(), CEILING);
        // 200K tokens at ~4 bytes each is 800 KB raw: the usable share is smaller.
        assert!(l.bytes < 800_000);
        assert!(l.bytes > 100_000, "no longer the old fixed 100 KB");
    }

    #[test]
    fn the_hard_ceiling_still_applies_to_a_huge_window() {
        let l = limit_for(
            &AgentType::GeminiCli,
            Some("gemini-2.5-pro"),
            &none(),
            1_000_000,
        );
        assert_eq!(l.bytes, 1_000_000);
        assert!(l.ceiling_bound);
        assert!(check(1_000_000, &l, "Prompt").is_ok());
        let refusal = check(1_000_001, &l, "Prompt").unwrap_err();
        assert!(refusal.contains("max_prompt_bytes"), "{refusal}");
        assert!(refusal.contains("gemini-2.5-pro"), "{refusal}");
    }

    #[test]
    fn a_lowered_ceiling_binds_even_a_model_that_would_fit() {
        let l = limit_for(&AgentType::ClaudeCode, None, &none(), 50_000);
        assert!(check(60_000, &l, "Message").is_err());
    }

    #[test]
    fn a_pinned_ollama_window_is_the_real_window() {
        let mut pinned = HashMap::new();
        pinned.insert("qwen3:27b".to_string(), 262_144u64);
        let l = limit_for(&AgentType::Ollama, Some("qwen3:27b"), &pinned, CEILING);
        assert_eq!(l.window_tokens, 262_144);
        assert!(check(500_000, &l, "Prompt").is_ok());
    }

    #[test]
    fn the_tightest_target_decides() {
        let targets = vec![
            MessageTarget::agent(AgentType::GeminiCli),
            MessageTarget::agent(AgentType::Ollama),
        ];
        let l = tightest_limit(
            &targets,
            &AgentType::ClaudeCode,
            ModelTier::Default,
            None,
            &ModelTiersConfig::default(),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 32_000);
    }

    #[test]
    fn the_discussion_model_pin_is_not_lent_to_another_agent() {
        // A Claude discussion pinned to a Claude model, one message also
        // addressed to Ollama: Ollama is sized on its own window.
        let targets = vec![
            MessageTarget::discussion_agent(AgentType::ClaudeCode),
            MessageTarget::agent(AgentType::Ollama),
        ];
        let l = tightest_limit(
            &targets,
            &AgentType::ClaudeCode,
            ModelTier::Default,
            Some("claude-sonnet-4-5"),
            &ModelTiersConfig::default(),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 32_000);
        assert!(check(500_000, &l, "Message").is_err());
    }

    #[test]
    fn the_discussion_model_pin_still_sizes_its_own_agent() {
        // Ollama names models freely: a pinned `num_ctx` is the only window.
        let l = tightest_limit(
            &[],
            &AgentType::Ollama,
            ModelTier::Default,
            Some("gemini-2.5-pro"),
            &ModelTiersConfig::default(),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 32_000);
        let l = tightest_limit(
            &[],
            &AgentType::LiteLlm,
            ModelTier::Default,
            Some("gemini-2.5-pro"),
            &ModelTiersConfig::default(),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 1_000_000);
    }

    #[test]
    fn openai_and_deepseek_families_are_recognised_through_a_gateway() {
        let window = |model: &str| {
            limit_for(&AgentType::LiteLlm, Some(model), &none(), CEILING).window_tokens
        };
        assert_eq!(window("gpt-4.1"), 1_000_000);
        assert_eq!(window("openai/gpt-4.1-mini"), 1_000_000);
        assert_eq!(window("gpt-5"), 200_000);
        assert_eq!(window("o3"), 200_000);
        assert_eq!(window("o4-mini"), 200_000);
        assert_eq!(window("gpt-4o"), 128_000);
        assert_eq!(window("deepseek-chat"), 128_000);
        assert_eq!(window("bedrock/anthropic.claude-3-5-sonnet"), 200_000);
        // Unknown name, or merely starting with an `o`: the runtime default.
        assert_eq!(window("ollama-thing"), 128_000);
        assert_eq!(window("o3000-custom"), 128_000);
    }

    #[test]
    fn a_family_name_inside_a_local_model_name_grants_nothing() {
        let l = limit_for(
            &AgentType::Ollama,
            Some("llama3-claude-distill:8b"),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 32_000);
        // Not even as a prefix: only a pinned `num_ctx` widens a local window.
        let l = limit_for(
            &AgentType::Ollama,
            Some("gemini-clone:7b"),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 32_000);
        // And a gateway model merely *containing* a family name is not it.
        let l = limit_for(
            &AgentType::LiteLlm,
            Some("acme/llama3-claude-distill"),
            &none(),
            CEILING,
        );
        assert_eq!(l.window_tokens, 128_000);
    }

    #[test]
    fn the_default_ceiling_is_a_few_megabytes() {
        assert!(check_hard_ceiling(3_900_000, DEFAULT_MAX_PROMPT_BYTES, "Prompt").is_ok());
        let refusal =
            check_hard_ceiling(4_000_001, DEFAULT_MAX_PROMPT_BYTES, "Prompt").unwrap_err();
        assert!(refusal.contains("4000000"), "{refusal}");
    }
}
