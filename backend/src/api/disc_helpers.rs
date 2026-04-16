//! Pure helpers extracted from `api::discussions`.
//!
//! These functions have no side effects (no AppState, no DB, no HTTP).
//! They live here so `discussions.rs` (~3400 lines) stops growing into a
//! single-file monolith and so the logic is testable in isolation.
//!
//! Rule of thumb: if you'd need `AppState` or `Connection` to run it, it
//! stays in `discussions.rs`. Otherwise it belongs here.

use crate::models::{AgentType, TokensConfig};

/// Per-agent prompt budget in characters.
/// Leaves room for the agent's response within its context window.
/// Conservative estimates — better to truncate safely than crash.
pub fn agent_prompt_budget(agent_type: &AgentType) -> usize {
    match agent_type {
        AgentType::ClaudeCode => 400_000, // ~100K tokens, 200K+ window
        AgentType::GeminiCli => 800_000,  // ~200K tokens, 1M window
        AgentType::Codex => 200_000,      // ~50K tokens, GPT-5 128K+ window
        AgentType::Kiro => 400_000,       // ~100K tokens, Claude via AWS Bedrock (200K window)
        AgentType::CopilotCli => 200_000, // ~50K tokens, GPT-4o 128K window
        AgentType::Vibe => 60_000,        // ~15K tokens, Mistral 128K window (API mode)
        AgentType::Ollama => 100_000,     // ~25K tokens, depends on model (llama3 128K window)
        AgentType::Custom => 60_000,      // reasonable default
    }
}

/// Resolve the auth mode string (`"override"` vs `"local"`) for a given
/// agent against the current tokens config. "override" = we have a user
/// key and it isn't disabled, so we'll short-circuit the CLI and hit the
/// provider API directly. "local" = we'll defer to the agent's own auth.
pub fn auth_mode_for(agent_type: &AgentType, tokens: &TokensConfig) -> String {
    let provider = match agent_type {
        AgentType::ClaudeCode => "anthropic",
        AgentType::Codex => "openai",
        AgentType::GeminiCli => "google",
        AgentType::Vibe => "mistral",
        AgentType::Kiro => "aws",
        AgentType::CopilotCli => "github",
        AgentType::Ollama => "ollama",
        AgentType::Custom => "",
    };
    let has_key = tokens.active_key_for(provider).is_some();
    let is_disabled = tokens.disabled_overrides.iter().any(|d| d == provider);
    if has_key && !is_disabled {
        "override".to_string()
    } else {
        "local".to_string()
    }
}

/// Human-readable agent name (used in summaries, attribution lines, etc.).
pub fn agent_display_name(agent_type: &AgentType) -> String {
    match agent_type {
        AgentType::ClaudeCode => "Claude Code".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::GeminiCli => "Gemini CLI".into(),
        AgentType::Kiro => "Kiro".into(),
        AgentType::CopilotCli => "GitHub Copilot".into(),
        AgentType::Ollama => "Ollama".into(),
        AgentType::Custom => "Custom".into(),
    }
}

/// Truncate text at the last sentence boundary before `max_len`, falling
/// back to the last word boundary. Uses `floor_char_boundary` to avoid
/// panicking on multi-byte UTF-8 (accents, emoji, CJK).
pub fn smart_truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let safe_len = text.floor_char_boundary(max_len);
    let slice = &text[..safe_len];
    if let Some(pos) = slice.rfind(['.', '!', '?']) {
        return text[..=pos].to_string();
    }
    if let Some(pos) = slice.rfind(' ') {
        return format!("{}…", &text[..pos]);
    }
    format!("{}…", slice)
}

/// Minimum non-System message count before a discussion becomes eligible
/// for summary generation. Tighter for small-budget agents because their
/// prompt gets full faster.
pub fn summary_msg_threshold(agent_type: &AgentType) -> u32 {
    let budget = agent_prompt_budget(agent_type);
    if budget >= 200_000 {
        12 // Large context (Claude Code, Kiro, Gemini)
    } else if budget >= 40_000 {
        8 // Medium context
    } else {
        4 // Small context (Codex, Vibe)
    }
}

/// Cooldown: minimum new messages since the last summary before we
/// re-summarize. Smaller for small-budget agents to keep the summary
/// fresh.
pub fn summary_cooldown(agent_type: &AgentType) -> u32 {
    let budget = agent_prompt_budget(agent_type);
    if budget >= 200_000 {
        6
    } else if budget >= 40_000 {
        4
    } else {
        2
    }
}

/// Agents with small context windows that need compact prompts
/// (strips verbose skill/profile bodies in favour of short bullet form).
pub fn is_compact_agent(agent_type: &AgentType) -> bool {
    matches!(
        agent_type,
        AgentType::Codex | AgentType::Kiro | AgentType::Vibe
    )
}

/// Language-lock header appended to agent prompts. Some models drift to
/// English without an explicit instruction, even when the conversation is
/// in another language.
pub fn language_instruction(lang: &str) -> &'static str {
    match lang {
        "fr" => "[IMPORTANT] Tu DOIS répondre en français. Toutes tes réponses doivent être en français.",
        "en" => "[IMPORTANT] You MUST respond in English. All your responses must be in English.",
        "es" => "[IMPORTANTE] DEBES responder en español. Todas tus respuestas deben ser en español.",
        "zh" => "[重要] 你必须用中文回答。你的所有回复都必须是中文。",
        "br" => "[POUEZUS] Ret eo dit respont e brezhoneg. Holl da respontoù a rank bezañ e brezhoneg.",
        _ => "[IMPORTANT] You MUST respond in English. All your responses must be in English.",
    }
}

/// Estimate the byte length of the extra context (skills + profiles +
/// directives + MCP) that will be prepended to the agent prompt. We need
/// this ahead of `build_agent_prompt` so the conversation-history budget
/// accounts for it and we don't overflow the agent's context window.
pub fn estimate_extra_context_len(
    skill_ids: &[String],
    directive_ids: &[String],
    profile_ids: &[String],
    project_path: &str,
    mcp_override: Option<&str>,
    agent_type: &AgentType,
) -> usize {
    let compact = is_compact_agent(agent_type);
    let profiles_len = if compact {
        crate::core::profiles::build_profiles_prompt_compact(profile_ids).len()
    } else {
        crate::core::profiles::build_profiles_prompt(profile_ids).len()
    };
    let skills_len = if compact {
        crate::core::skills::build_skills_prompt_compact(skill_ids).len()
    } else {
        crate::core::skills::build_skills_prompt(skill_ids).len()
    };
    let directives_len = crate::core::directives::build_directives_prompt(directive_ids).len();
    let mcp_len = if let Some(ctx) = mcp_override {
        ctx.len()
    } else if !project_path.is_empty() {
        crate::core::mcp_scanner::read_all_mcp_contexts(project_path).len()
    } else {
        0
    };
    // Add separators between non-empty parts.
    profiles_len + skills_len + directives_len + mcp_len + 20
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ApiKey;

    fn tokens_with_key(provider: &str) -> TokensConfig {
        TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: vec![ApiKey {
                id: "k1".into(),
                name: "default".into(),
                provider: provider.into(),
                value: "sk-test".into(),
                active: true,
            }],
            disabled_overrides: vec![],
        }
    }

    fn tokens_empty() -> TokensConfig {
        TokensConfig {
            anthropic: None,
            openai: None,
            google: None,
            keys: vec![],
            disabled_overrides: vec![],
        }
    }

    #[test]
    fn budget_matches_context_tiers() {
        // Sanity: known tiers line up with the summary threshold function
        // (tests below depend on this ordering, so lock it in here).
        // NOTE: despite the docstring calling Vibe/Custom "small-budget",
        // all current agents land in tier >= 40_000 — no agent hits the
        // <40_000 branch today. Tests encode the actual behaviour.
        assert!(agent_prompt_budget(&AgentType::GeminiCli) >= 200_000);
        assert!(agent_prompt_budget(&AgentType::ClaudeCode) >= 200_000);
        assert!(agent_prompt_budget(&AgentType::Codex) >= 200_000);
        assert!((40_000..200_000).contains(&agent_prompt_budget(&AgentType::Vibe)));
        assert!((40_000..200_000).contains(&agent_prompt_budget(&AgentType::Custom)));
    }

    #[test]
    fn auth_mode_override_when_key_active_and_not_disabled() {
        let tokens = tokens_with_key("anthropic");
        assert_eq!(auth_mode_for(&AgentType::ClaudeCode, &tokens), "override");
    }

    #[test]
    fn auth_mode_local_when_no_key() {
        let tokens = tokens_empty();
        assert_eq!(auth_mode_for(&AgentType::ClaudeCode, &tokens), "local");
    }

    #[test]
    fn auth_mode_local_when_provider_disabled() {
        let mut tokens = tokens_with_key("anthropic");
        tokens.disabled_overrides.push("anthropic".into());
        assert_eq!(auth_mode_for(&AgentType::ClaudeCode, &tokens), "local");
    }

    #[test]
    fn auth_mode_custom_agent_falls_back_to_local() {
        // Custom agents have no provider mapping; treat as local.
        let tokens = tokens_with_key("anything");
        assert_eq!(auth_mode_for(&AgentType::Custom, &tokens), "local");
    }

    #[test]
    fn display_names_are_human_readable() {
        assert_eq!(agent_display_name(&AgentType::ClaudeCode), "Claude Code");
        assert_eq!(agent_display_name(&AgentType::GeminiCli), "Gemini CLI");
        assert_eq!(agent_display_name(&AgentType::CopilotCli), "GitHub Copilot");
    }

    #[test]
    fn smart_truncate_returns_original_if_short_enough() {
        assert_eq!(smart_truncate("hello", 100), "hello");
    }

    #[test]
    fn smart_truncate_prefers_sentence_boundary() {
        let s = "First sentence. Second sentence that is longer.";
        let out = smart_truncate(s, 20);
        assert!(out.ends_with('.'), "got: {:?}", out);
        assert_eq!(out, "First sentence.");
    }

    #[test]
    fn smart_truncate_falls_back_to_word_boundary() {
        let s = "word1 word2 word3 word4 word5";
        let out = smart_truncate(s, 15);
        assert!(out.ends_with('…'), "got: {:?}", out);
        assert!(!out.contains("word4"), "got: {:?}", out);
    }

    #[test]
    fn smart_truncate_never_panics_on_multibyte_boundary() {
        // Regression: slicing at a computed byte index used to panic on
        // French/emoji strings. The fix uses `floor_char_boundary`.
        let s = "Résumé: café ☕ complété avec succès 🎉 voilà.";
        for len in 1..s.len() {
            let _ = smart_truncate(s, len);
        }
    }

    #[test]
    fn summary_threshold_scales_with_budget() {
        // Tier 1 (>=200K): Claude, Gemini, Codex, Kiro
        assert_eq!(summary_msg_threshold(&AgentType::GeminiCli), 12);
        assert_eq!(summary_msg_threshold(&AgentType::ClaudeCode), 12);
        assert_eq!(summary_msg_threshold(&AgentType::Codex), 12);
        // Tier 2 (>=40K): Ollama, Vibe, Custom
        assert_eq!(summary_msg_threshold(&AgentType::Ollama), 8);
        assert_eq!(summary_msg_threshold(&AgentType::Vibe), 8);
        assert_eq!(summary_msg_threshold(&AgentType::Custom), 8);
    }

    #[test]
    fn summary_cooldown_scales_with_budget() {
        assert_eq!(summary_cooldown(&AgentType::GeminiCli), 6);
        assert_eq!(summary_cooldown(&AgentType::ClaudeCode), 6);
        assert_eq!(summary_cooldown(&AgentType::Ollama), 4);
        assert_eq!(summary_cooldown(&AgentType::Vibe), 4);
    }

    #[test]
    fn is_compact_agent_flags_small_context_agents() {
        assert!(is_compact_agent(&AgentType::Codex));
        assert!(is_compact_agent(&AgentType::Kiro));
        assert!(is_compact_agent(&AgentType::Vibe));
        assert!(!is_compact_agent(&AgentType::ClaudeCode));
        assert!(!is_compact_agent(&AgentType::GeminiCli));
    }

    #[test]
    fn language_instruction_known_locales() {
        assert!(language_instruction("fr").contains("français"));
        assert!(language_instruction("en").contains("English"));
        assert!(language_instruction("es").contains("español"));
    }

    #[test]
    fn language_instruction_unknown_falls_back_to_english() {
        // Regression: defensive default matters — an unknown lang used to
        // leak through as an empty string, letting the model drift.
        assert!(language_instruction("xx").contains("English"));
        assert!(language_instruction("").contains("English"));
    }
}
