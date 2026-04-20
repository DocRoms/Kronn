//! Pure prompt builders extracted from `api::discussions`.
//!
//! Three public entry points:
//! - [`build_agent_prompt`] — serialise a discussion's history into the
//!   prompt a single agent sees (with summary injection + budget-aware
//!   truncation).
//! - [`build_orchestration_prompt`] — one debate round's prompt for a
//!   specific agent, across up to 3 locales (fr/es/en-default).
//! - [`build_synthesis_prompt`] — final-round synthesis prompt that
//!   collapses the debate into agreements/disagreements/recommendation.
//!
//! These are all pure functions: they take refs and return owned
//! `String`s. No side effects, no async, no DB. Centralising them here
//! keeps `discussions.rs` focused on the handler/SSE plumbing.
//!
//! See `disc_helpers.rs` for the small text/agent utilities reused here.

use crate::models::{AgentType, Discussion, MessageRole};

use super::disc_helpers::{
    agent_display_name, agent_prompt_budget, language_instruction, smart_truncate,
};

/// Per-round debate context fed to [`build_orchestration_prompt`].
///
/// Lifetimes: all `&'a` refs borrow from the orchestration driver — the
/// builder only needs them for the duration of its call and never holds
/// on to them past the returned `String`.
pub struct OrchestrationContext<'a> {
    pub question: &'a str,
    pub current_agent: &'a AgentType,
    pub all_agents: &'a [String],
    pub previous_rounds: &'a [Vec<(String, String)>],
    pub round: u32,
    pub max_rounds: u32,
    pub lang: &'a str,
    pub conversation_context: &'a str,
}

pub fn build_orchestration_prompt(ctx: &OrchestrationContext) -> String {
    let agent_name = agent_display_name(ctx.current_agent);

    // Conversation context section (prior exchanges before the debated question)
    let conv_section = if ctx.conversation_context.is_empty() {
        String::new()
    } else {
        match ctx.lang {
            "fr" => format!("Contexte de la conversation precedente (ne pas repeter) :\n\n{}\n\n", ctx.conversation_context),
            "es" => format!("Contexto de la conversacion anterior (no repetir) :\n\n{}\n\n", ctx.conversation_context),
            _ => format!("Previous conversation context (do not repeat) :\n\n{}\n\n", ctx.conversation_context),
        }
    };

    if ctx.round == 1 {
        match ctx.lang {
            "fr" => format!(
                "Tu es {} dans un debat technique entre agents IA ({}).\n\
                {}\
                Donne ton point de vue unique sur la question ci-dessous.\n\
                Sois concis et precis (max 200 mots). Ne repete PAS la question.\n\
                Concentre-toi sur ton expertise specifique.\n\
                Reponds en francais.\n\n\
                Question : {}",
                agent_name, ctx.all_agents.join(", "), conv_section, ctx.question
            ),
            "es" => format!(
                "Eres {} en un debate tecnico entre agentes IA ({}).\n\
                {}\
                Da tu perspectiva unica sobre la pregunta.\n\
                Se conciso y preciso (max 200 palabras). NO repitas la pregunta.\n\
                Responde en espanol.\n\n\
                Pregunta: {}",
                agent_name, ctx.all_agents.join(", "), conv_section, ctx.question
            ),
            _ => format!(
                "You are {} in a technical debate between AI agents ({}).\n\
                {}\
                Give your unique perspective on the question below.\n\
                Be concise and precise (max 200 words). Do NOT repeat the question.\n\
                Focus on your specific expertise and what you uniquely bring.\n\
                Respond in English.\n\n\
                Question: {}",
                agent_name, ctx.all_agents.join(", "), conv_section, ctx.question
            ),
        }
    } else {
        let mut prompt = match ctx.lang {
            "fr" => format!(
                "Tu es {} au round {}/{} d'un debat technique ({}).\n\
                Voici les echanges precedents :\n\n",
                agent_name, ctx.round, ctx.max_rounds, ctx.all_agents.join(", ")
            ),
            "es" => format!(
                "Eres {} en la ronda {}/{} de un debate tecnico ({}).\n\
                Intercambios anteriores:\n\n",
                agent_name, ctx.round, ctx.max_rounds, ctx.all_agents.join(", ")
            ),
            _ => format!(
                "You are {} in round {}/{} of a technical debate ({}).\n\
                Here are the previous exchanges:\n\n",
                agent_name, ctx.round, ctx.max_rounds, ctx.all_agents.join(", ")
            ),
        };

        if !ctx.conversation_context.is_empty() {
            prompt.push_str(&conv_section);
        }

        for (r_idx, round_data) in ctx.previous_rounds.iter().enumerate() {
            prompt.push_str(&format!("--- Round {} ---\n", r_idx + 1));
            for (name, response) in round_data {
                let truncated = smart_truncate(response, 500);
                prompt.push_str(&format!("{}: {}\n\n", name, truncated));
            }
        }

        match ctx.lang {
            "fr" => prompt.push_str(&format!(
                "Question originale : {}\n\n\
                REGLES IMPORTANTES :\n\
                - Ne repete PAS ce que les autres ont dit. Ne resume PAS les rounds precedents.\n\
                - Ne parle QUE si tu as quelque chose de NOUVEAU : un desaccord, une nuance, une correction.\n\
                - Si tu es d'accord avec tout, reponds juste : \"Je suis d'accord avec le consensus.\" et arrete-toi.\n\
                - Si c'est le round {}/{}, donne ta position FINALE en 1-2 phrases.\n\
                - Max 150 mots.\n\
                Reponds en francais.",
                ctx.question, ctx.round, ctx.max_rounds
            )),
            "es" => prompt.push_str(&format!(
                "Pregunta original: {}\n\n\
                REGLAS IMPORTANTES:\n\
                - NO repitas lo que otros dijeron. NO resumas rondas anteriores.\n\
                - Solo habla si tienes algo NUEVO: un desacuerdo, un matiz, una correccion.\n\
                - Si estas de acuerdo con todo, responde: \"Estoy de acuerdo con el consenso.\" y para.\n\
                - Si es la ronda {}/{}, da tu posicion FINAL en 1-2 frases.\n\
                - Max 150 palabras.\n\
                Responde en espanol.",
                ctx.question, ctx.round, ctx.max_rounds
            )),
            _ => prompt.push_str(&format!(
                "Original question: {}\n\n\
                IMPORTANT RULES:\n\
                - Do NOT repeat what others said. Do NOT summarize previous rounds.\n\
                - Only speak if you have something NEW to add: a disagreement, a nuance, a correction.\n\
                - If you agree with everything said, just state: \"I agree with the consensus.\" and stop.\n\
                - If this is round {}/{}, give your FINAL position in 1-2 sentences.\n\
                - Max 150 words.\n\
                Respond in English.",
                ctx.question, ctx.round, ctx.max_rounds
            )),
        }
        prompt
    }
}

pub fn build_synthesis_prompt(
    question: &str,
    all_rounds: &[Vec<(String, String)>],
    lang: &str,
) -> String {
    let mut ctx = match lang {
        "fr" => format!(
            "Tu synthetises un debat technique entre agents IA.\n\n\
            Question : {}\n\n",
            question
        ),
        "es" => format!(
            "Sintetizas un debate tecnico entre agentes IA.\n\n\
            Pregunta: {}\n\n",
            question
        ),
        _ => format!(
            "You are synthesizing a technical debate between AI agents.\n\n\
            Question: {}\n\n",
            question
        ),
    };

    let initial_label = match lang {
        "fr" => "--- Positions initiales ---",
        "es" => "--- Posiciones iniciales ---",
        _ => "--- Initial positions ---",
    };
    let final_label = match lang {
        "fr" => format!("--- Positions finales (round {}) ---", all_rounds.len()),
        "es" => format!("--- Posiciones finales (ronda {}) ---", all_rounds.len()),
        _ => format!("--- Final positions (round {}) ---", all_rounds.len()),
    };

    if let Some(first) = all_rounds.first() {
        ctx.push_str(&format!("{}\n", initial_label));
        for (name, response) in first {
            ctx.push_str(&format!("{}: {}\n\n", name, smart_truncate(response, 400)));
        }
    }
    if all_rounds.len() > 1 {
        if let Some(last) = all_rounds.last() {
            ctx.push_str(&format!("{}\n", final_label));
            for (name, response) in last {
                ctx.push_str(&format!("{}: {}\n\n", name, smart_truncate(response, 400)));
            }
        }
    }

    match lang {
        "fr" => ctx.push_str(
            "Produis une synthese claire et actionnable :\n\
            1. Points d'ACCORD (convergences entre tous les agents)\n\
            2. DESACCORDS restants (s'il y en a)\n\
            3. RECOMMANDATION FINALE\n\
            Sois concis et structure. Reponds en francais."
        ),
        "es" => ctx.push_str(
            "Produce una sintesis clara y accionable:\n\
            1. Puntos de ACUERDO (convergencias entre todos los agentes)\n\
            2. DESACUERDOS restantes (si los hay)\n\
            3. RECOMENDACION FINAL\n\
            Se conciso y estructurado. Responde en espanol."
        ),
        _ => ctx.push_str(
            "Produce a clear, actionable synthesis:\n\
            1. Points of AGREEMENT (what all agents converge on)\n\
            2. Remaining DISAGREEMENTS (if any)\n\
            3. FINAL RECOMMENDATION\n\
            Be concise and structured. Respond in English."
        ),
    }
    ctx
}

/// Build the agent prompt with conversation history, respecting the
/// agent's prompt budget.
///
/// Strategy: always include the latest user message. Then fill backwards
/// from recent messages until we hit the budget. If older messages are
/// truncated, prepend a notice. `extra_context_len` is the size of
/// profiles + skills + directives + MCP that will be added alongside
/// this prompt (so we don't exceed the agent's total budget).
/// Notice injected when the discussion runs in an isolated git worktree.
///
/// Without it, agents (especially Claude Code) touch files but don't commit —
/// the branch stays at the base commit and the user sees nothing when they
/// check out the branch from outside the worktree. The notice names the branch
/// explicitly and asks for a final commit, which gets the default behavior
/// right in ~80% of runs. The UI badge on the git-panel icon is the safety
/// net for the remaining cases.
fn isolated_worktree_notice(disc: &Discussion) -> String {
    if disc.workspace_mode != "Isolated" {
        return String::new();
    }
    let branch = match disc.worktree_branch.as_deref() {
        Some(b) if !b.is_empty() => b,
        _ => return String::new(),
    };
    match disc.language.as_str() {
        "fr" => format!(
            "[ISOLATION GIT — branche dédiée]\n\
             Tu travailles dans un worktree sur la branche `{}`. Toute \
             modification de fichier reste locale au worktree tant que tu ne \
             commits pas. Après avoir terminé tes modifications :\n\
             1. `git status` pour lister les fichiers touchés\n\
             2. `git add <fichiers>` (ou `git add -A` si tout est pertinent)\n\
             3. `git commit -m \"<message descriptif>\"`\n\
             Sans ce commit, la branche reste vide côté utilisateur. Ne push pas \
             sauf demande explicite.\n\n",
            branch
        ),
        "es" => format!(
            "[AISLAMIENTO GIT — rama dedicada]\n\
             Trabajas en un worktree en la rama `{}`. Cualquier modificación \
             de archivo queda local al worktree hasta que hagas commit. Al \
             terminar tus cambios :\n\
             1. `git status` para listar los archivos modificados\n\
             2. `git add <archivos>` (o `git add -A` si todo es relevante)\n\
             3. `git commit -m \"<mensaje descriptivo>\"`\n\
             Sin este commit, la rama permanece vacía para el usuario. No hagas \
             push salvo petición explícita.\n\n",
            branch
        ),
        _ => format!(
            "[GIT ISOLATION — dedicated branch]\n\
             You are working in a worktree on branch `{}`. File modifications \
             stay local to the worktree until you commit. Once your changes \
             are done:\n\
             1. `git status` to list touched files\n\
             2. `git add <files>` (or `git add -A` if all relevant)\n\
             3. `git commit -m \"<descriptive message>\"`\n\
             Without this commit, the branch stays empty from the user's \
             perspective. Do not push unless explicitly asked.\n\n",
            branch
        ),
    }
}

pub fn build_agent_prompt(
    disc: &Discussion,
    agent_type: &AgentType,
    extra_context_len: usize,
) -> String {
    let budget = agent_prompt_budget(agent_type).saturating_sub(extra_context_len);
    let lang_instr = language_instruction(&disc.language);

    // Include discussion title as context if it's meaningful (not auto-generated placeholder)
    let title_label = match disc.language.as_str() {
        "fr" => "Sujet de la discussion",
        "es" => "Tema de la discusión",
        _ => "Discussion topic",
    };
    let title_ctx = if !disc.title.is_empty()
        && disc.title != "New discussion"
        && disc.title != "Nouvelle discussion"
        && !disc.title.starts_with("Bootstrap: ")
    {
        format!("{}: \"{}\"\n\n", title_label, disc.title)
    } else {
        String::new()
    };

    let worktree_notice = isolated_worktree_notice(disc);

    let user_msgs: Vec<_> = disc
        .messages
        .iter()
        .filter(|m| matches!(m.role, MessageRole::User))
        .collect();

    if user_msgs.len() <= 1 {
        let content = user_msgs.last().map(|m| m.content.clone()).unwrap_or_default();
        // Language instruction at end only — LLMs weight recent text more heavily,
        // and MCP context is injected via --append-system-prompt (separate from prompt).
        return format!("{}{}{}\n\n{}", title_ctx, worktree_notice, content, lang_instr);
    }

    // Fixed overhead: header + footer (localized by discussion language)
    let prev_conv_label = match disc.language.as_str() {
        "fr" => "Conversation précédente :\n\n",
        "es" => "Conversación anterior:\n\n",
        _ => "Previous conversation:\n\n",
    };
    let footer = match disc.language.as_str() {
        "fr" => "Répondez au dernier message ci-dessus. Reponds en francais.",
        "es" => "Responda al último mensaje anterior. Responda en español.",
        "zh" => "请回复上面的最新用户消息。请用中文回答。",
        "br" => "Respontet d'ar c'hemenn diwezhañ a-us. Respont e brezhoneg.",
        _ => "Please respond to the latest user message above. Respond in English.",
    };
    // For agents that think they're in non-interactive mode (Gemini -p, Codex exec),
    // clarify that this IS a multi-turn conversation managed by Kronn.
    // Always include for pinned discussions (briefing/validation/bootstrap) since
    // agents like Gemini detect -p mode and refuse to interact on the first message.
    let interactive_hint = if user_msgs.len() > 1 || disc.pin_first_message {
        match disc.language.as_str() {
            "fr" => "NOTE: Tu es dans une conversation multi-tours geree par Kronn. Tu PEUX poser des questions et attendre des reponses. Chaque message te sera transmis avec l'historique complet.\n\n",
            "es" => "NOTA: Estas en una conversacion multi-turno gestionada por Kronn. PUEDES hacer preguntas y esperar respuestas. Cada mensaje te sera transmitido con el historial completo.\n\n",
            _ => "NOTE: You are in a multi-turn conversation managed by Kronn. You CAN ask questions and wait for answers. Each message will be sent to you with the full history.\n\n",
        }
    } else {
        ""
    };

    let header = format!("{}{}{}{}", title_ctx, worktree_notice, interactive_hint, prev_conv_label);
    let overhead = header.len() + footer.len() + 100; // 100 = notice template space

    // If pin_first_message is set, extract and pin the first non-system message
    let non_system_msgs: Vec<_> = disc
        .messages
        .iter()
        .filter(|m| !matches!(m.role, MessageRole::System))
        .collect();

    let pinned_block = if disc.pin_first_message {
        non_system_msgs
            .first()
            .map(|msg| {
                format!(
                    "[INSTRUCTIONS DU PROTOCOLE — ne pas ignorer]\n{}\n[FIN INSTRUCTIONS]\n\n",
                    msg.content
                )
            })
            .unwrap_or_default()
    } else {
        String::new()
    };

    // If we have a cached summary, inject it and only include messages after the summary
    let summary_block = if let Some(ref summary) = disc.summary_cache {
        let idx = disc.summary_up_to_msg_idx.unwrap_or(0) as usize;
        match disc.language.as_str() {
            "fr" => format!("Résumé de la conversation précédente (messages 1-{}) :\n{}\n\n", idx, summary),
            "es" => format!("Resumen de la conversación anterior (mensajes 1-{}):\n{}\n\n", idx, summary),
            _ => format!("Summary of earlier conversation (messages 1-{}):\n{}\n\n", idx, summary),
        }
    } else {
        String::new()
    };

    let remaining_budget = budget.saturating_sub(overhead + pinned_block.len() + summary_block.len());

    // Format messages (skip System). When a summary exists, skip messages already covered.
    // When pin_first_message is set, skip index 0 (it's already pinned above).
    let summary_covers_up_to = if disc.summary_cache.is_some() {
        disc.summary_up_to_msg_idx.unwrap_or(0) as usize
    } else {
        0
    };
    let skip_pinned = if disc.pin_first_message { 1 } else { 0 };
    let skip_from = summary_covers_up_to.max(skip_pinned);
    let formatted_msgs: Vec<String> = non_system_msgs
        .iter()
        .enumerate()
        .filter(|(i, _)| *i >= skip_from)
        .map(|(_, msg)| match msg.role {
            MessageRole::User => format!("User: {}\n\n", msg.content),
            MessageRole::Agent => {
                let agent_label = msg
                    .agent_type
                    .as_ref()
                    .map(agent_display_name)
                    .unwrap_or_else(|| "Agent".into());
                format!("{}: {}\n\n", agent_label, msg.content)
            }
            MessageRole::System => unreachable!(),
        })
        .collect();

    // Always include the last message (latest user prompt). Walk backwards to fill budget.
    let total_msgs = formatted_msgs.len();
    let mut included_from_end = 0;
    let mut cumulative_len = 0;

    for msg in formatted_msgs.iter().rev() {
        if cumulative_len + msg.len() > remaining_budget && included_from_end > 0 {
            break;
        }
        cumulative_len += msg.len();
        included_from_end += 1;
    }

    let start_idx = total_msgs - included_from_end;
    let omitted_count = start_idx;

    let mut prompt = header;

    // Inject pinned message (protocol prompt) before everything else
    if !pinned_block.is_empty() {
        prompt.push_str(&pinned_block);
    }

    // Inject summary if available
    if !summary_block.is_empty() {
        prompt.push_str(&summary_block);
    }

    if omitted_count > 0 {
        let has_summary = !summary_block.is_empty();
        let omitted_notice = match disc.language.as_str() {
            "fr" => format!(
                "════════════════════════════════════════\n\
                 CONTEXTE LIMITE : {} messages anterieurs non inclus{}\n\
                 ════════════════════════════════════════\n\n",
                omitted_count,
                if has_summary { " (resume ci-dessus)" } else { " — demandez a l'utilisateur si besoin" }
            ),
            "es" => format!(
                "════════════════════════════════════════\n\
                 CONTEXTO LIMITADO: {} mensajes anteriores no incluidos{}\n\
                 ════════════════════════════════════════\n\n",
                omitted_count,
                if has_summary { " (resumen arriba)" } else { " — pregunte al usuario si necesario" }
            ),
            _ => format!(
                "════════════════════════════════════════\n\
                 CONTEXT LIMITED: {} earlier messages not included{}\n\
                 ════════════════════════════════════════\n\n",
                omitted_count,
                if has_summary { " (see summary above)" } else { " — ask user to recap if needed" }
            ),
        };
        prompt.push_str(&omitted_notice);
    }

    if omitted_count > 0 {
        tracing::info!(
            "Prompt truncation: {} of {} messages omitted for {:?} (budget: {} chars, has_summary: {})",
            omitted_count, total_msgs, agent_type, budget, !summary_block.is_empty()
        );
    }

    for msg in &formatted_msgs[start_idx..] {
        prompt.push_str(msg);
    }

    prompt.push_str(footer);
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Discussion, DiscussionMessage, MessageRole, ModelTier};

    fn disc_with_messages(messages: Vec<DiscussionMessage>, language: &str) -> Discussion {
        Discussion {
            id: "d-test".into(),
            project_id: None,
            title: "Test discussion".into(),
            agent: AgentType::ClaudeCode,
            language: language.into(),
            participants: vec![],
            messages,
            message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: ModelTier::Default,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            shared_id: None,
            shared_with: vec![],
            workflow_run_id: None,
            test_mode_restore_branch: None,
            test_mode_stash_ref: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    fn user_msg(content: &str) -> DiscussionMessage {
        DiscussionMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::User,
            content: content.into(),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
        }
    }

    #[test]
    fn agent_prompt_single_user_message_uses_short_form() {
        // One user message → no "Previous conversation" header, just content + lang instruction.
        let disc = disc_with_messages(vec![user_msg("Hello Claude")], "en");
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("Hello Claude"));
        assert!(prompt.contains("MUST respond in English"));
        assert!(!prompt.contains("Previous conversation"));
    }

    #[test]
    fn agent_prompt_multi_message_includes_history_and_footer() {
        let msgs = vec![
            user_msg("first question"),
            user_msg("follow-up question"),
        ];
        let disc = disc_with_messages(msgs, "en");
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("Previous conversation"));
        assert!(prompt.contains("first question"));
        assert!(prompt.contains("follow-up question"));
        assert!(prompt.contains("Please respond to the latest user message"));
    }

    #[test]
    fn agent_prompt_localized_footer_matches_language() {
        let msgs = vec![user_msg("salut"), user_msg("suite")];
        let disc_fr = disc_with_messages(msgs.clone(), "fr");
        let prompt_fr = build_agent_prompt(&disc_fr, &AgentType::ClaudeCode, 0);
        assert!(prompt_fr.contains("Conversation précédente"));
        assert!(prompt_fr.contains("Reponds en francais"));

        let disc_es = disc_with_messages(msgs, "es");
        let prompt_es = build_agent_prompt(&disc_es, &AgentType::ClaudeCode, 0);
        assert!(prompt_es.contains("Conversación anterior"));
        assert!(prompt_es.contains("Responda en español"));
    }

    #[test]
    fn agent_prompt_placeholder_title_does_not_leak() {
        // Titles like "New discussion" shouldn't leak into the prompt —
        // they're a UI artefact, not real context the agent needs.
        let mut disc = disc_with_messages(vec![user_msg("hi")], "en");
        disc.title = "New discussion".into();
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(!prompt.contains("Discussion topic"));
    }

    #[test]
    fn agent_prompt_isolated_mode_injects_worktree_notice_short_form() {
        // Single-message path: notice must be present with the branch name when Isolated.
        let mut disc = disc_with_messages(vec![user_msg("add a feature")], "en");
        disc.workspace_mode = "Isolated".into();
        disc.worktree_branch = Some("kronn/add-feature".into());
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("GIT ISOLATION"), "notice heading missing: {}", prompt);
        assert!(prompt.contains("kronn/add-feature"), "branch name missing: {}", prompt);
        assert!(prompt.contains("git commit"), "commit instruction missing");
    }

    #[test]
    fn agent_prompt_isolated_mode_injects_worktree_notice_multi_form() {
        // Multi-message path: notice must land in the header (before "Previous conversation").
        let msgs = vec![user_msg("first"), user_msg("follow-up")];
        let mut disc = disc_with_messages(msgs, "fr");
        disc.workspace_mode = "Isolated".into();
        disc.worktree_branch = Some("kronn/ui-theme".into());
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("ISOLATION GIT"));
        assert!(prompt.contains("kronn/ui-theme"));
        // Notice must precede the conversation history to set expectations early.
        let notice_pos = prompt.find("ISOLATION GIT").unwrap();
        let conv_pos = prompt.find("Conversation précédente").unwrap();
        assert!(notice_pos < conv_pos, "worktree notice should precede conversation");
    }

    #[test]
    fn agent_prompt_direct_mode_omits_worktree_notice() {
        // Default workspace_mode = "Direct" → no notice injected.
        let disc = disc_with_messages(vec![user_msg("hello")], "en");
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(!prompt.contains("GIT ISOLATION"));
        assert!(!prompt.contains("worktree"));
    }

    #[test]
    fn agent_prompt_isolated_without_branch_skips_notice() {
        // Defensive path: workspace_mode set to Isolated but branch missing
        // (e.g. mid-migration, broken DB row) → no notice, no panic.
        let mut disc = disc_with_messages(vec![user_msg("hello")], "en");
        disc.workspace_mode = "Isolated".into();
        disc.worktree_branch = None;
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(!prompt.contains("GIT ISOLATION"));
    }

    #[test]
    fn agent_prompt_budget_truncates_old_messages() {
        // Deliberately oversize: fill so that old messages must be omitted.
        // Using Vibe (budget 60_000 chars) + extra_context_len eating most of it
        // forces the truncation path.
        let big = "x".repeat(5_000);
        let msgs = vec![
            user_msg(&big),
            user_msg(&big),
            user_msg(&big),
            user_msg(&big),
            user_msg(&big),
            user_msg("final"),
        ];
        let disc = disc_with_messages(msgs, "en");
        let prompt = build_agent_prompt(&disc, &AgentType::Vibe, 50_000);
        // The latest "final" message must always be included.
        assert!(prompt.contains("final"));
        // At least one old message should have been dropped — look for the notice.
        assert!(
            prompt.contains("CONTEXT LIMITED"),
            "expected truncation notice in prompt"
        );
    }

    #[test]
    fn orchestration_prompt_round_one_asks_unique_perspective() {
        let agents = ["Claude Code".to_string(), "Codex".to_string()];
        let ctx = OrchestrationContext {
            question: "Should we ship?",
            current_agent: &AgentType::ClaudeCode,
            all_agents: &agents,
            previous_rounds: &[],
            round: 1,
            max_rounds: 2,
            lang: "en",
            conversation_context: "",
        };
        let prompt = build_orchestration_prompt(&ctx);
        assert!(prompt.contains("Should we ship?"));
        assert!(prompt.contains("Claude Code"));
        assert!(prompt.contains("unique perspective"));
        // No "previous exchanges" in round 1.
        assert!(!prompt.contains("previous exchanges"));
    }

    #[test]
    fn orchestration_prompt_round_two_includes_prior_rounds() {
        let r1 = vec![
            ("Claude Code".into(), "ship it".into()),
            ("Codex".into(), "more tests first".into()),
        ];
        let agents = ["Claude Code".to_string(), "Codex".to_string()];
        let ctx = OrchestrationContext {
            question: "Should we ship?",
            current_agent: &AgentType::Codex,
            all_agents: &agents,
            previous_rounds: &[r1],
            round: 2,
            max_rounds: 2,
            lang: "en",
            conversation_context: "",
        };
        let prompt = build_orchestration_prompt(&ctx);
        assert!(prompt.contains("--- Round 1 ---"));
        assert!(prompt.contains("ship it"));
        assert!(prompt.contains("more tests first"));
        assert!(prompt.contains("IMPORTANT RULES"));
    }

    #[test]
    fn synthesis_prompt_includes_initial_and_final_positions() {
        let r1 = vec![("A".into(), "init-a".into()), ("B".into(), "init-b".into())];
        let r2 = vec![("A".into(), "final-a".into()), ("B".into(), "final-b".into())];
        let prompt = build_synthesis_prompt("Q?", &[r1, r2], "en");
        assert!(prompt.contains("Initial positions"));
        assert!(prompt.contains("Final positions (round 2)"));
        assert!(prompt.contains("init-a"));
        assert!(prompt.contains("final-b"));
        assert!(prompt.contains("AGREEMENT"));
    }

    #[test]
    fn synthesis_prompt_single_round_skips_final_section() {
        let r1 = vec![("A".into(), "only-a".into())];
        let prompt = build_synthesis_prompt("Q?", &[r1], "en");
        assert!(prompt.contains("Initial positions"));
        assert!(!prompt.contains("Final positions"));
    }
}
