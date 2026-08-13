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

use crate::models::{AgentType, Discussion, DiscussionMessage, MessageRole};

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
            "fr" => format!(
                "Contexte de la conversation precedente (ne pas repeter) :\n\n{}\n\n",
                ctx.conversation_context
            ),
            "es" => format!(
                "Contexto de la conversacion anterior (no repetir) :\n\n{}\n\n",
                ctx.conversation_context
            ),
            _ => format!(
                "Previous conversation context (do not repeat) :\n\n{}\n\n",
                ctx.conversation_context
            ),
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
                agent_name,
                ctx.all_agents.join(", "),
                conv_section,
                ctx.question
            ),
            "es" => format!(
                "Eres {} en un debate tecnico entre agentes IA ({}).\n\
                {}\
                Da tu perspectiva unica sobre la pregunta.\n\
                Se conciso y preciso (max 200 palabras). NO repitas la pregunta.\n\
                Responde en espanol.\n\n\
                Pregunta: {}",
                agent_name,
                ctx.all_agents.join(", "),
                conv_section,
                ctx.question
            ),
            _ => format!(
                "You are {} in a technical debate between AI agents ({}).\n\
                {}\
                Give your unique perspective on the question below.\n\
                Be concise and precise (max 200 words). Do NOT repeat the question.\n\
                Focus on your specific expertise and what you uniquely bring.\n\
                Respond in English.\n\n\
                Question: {}",
                agent_name,
                ctx.all_agents.join(", "),
                conv_section,
                ctx.question
            ),
        }
    } else {
        let mut prompt = match ctx.lang {
            "fr" => format!(
                "Tu es {} au round {}/{} d'un debat technique ({}).\n\
                Voici les echanges precedents :\n\n",
                agent_name,
                ctx.round,
                ctx.max_rounds,
                ctx.all_agents.join(", ")
            ),
            "es" => format!(
                "Eres {} en la ronda {}/{} de un debate tecnico ({}).\n\
                Intercambios anteriores:\n\n",
                agent_name,
                ctx.round,
                ctx.max_rounds,
                ctx.all_agents.join(", ")
            ),
            _ => format!(
                "You are {} in round {}/{} of a technical debate ({}).\n\
                Here are the previous exchanges:\n\n",
                agent_name,
                ctx.round,
                ctx.max_rounds,
                ctx.all_agents.join(", ")
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
            Sois concis et structure. Reponds en francais.",
        ),
        "es" => ctx.push_str(
            "Produce una sintesis clara y accionable:\n\
            1. Puntos de ACUERDO (convergencias entre todos los agentes)\n\
            2. DESACUERDOS restantes (si los hay)\n\
            3. RECOMENDACION FINAL\n\
            Se conciso y estructurado. Responde en espanol.",
        ),
        _ => ctx.push_str(
            "Produce a clear, actionable synthesis:\n\
            1. Points of AGREEMENT (what all agents converge on)\n\
            2. Remaining DISAGREEMENTS (if any)\n\
            3. FINAL RECOMMENDATION\n\
            Be concise and structured. Respond in English.",
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

fn reply_context(message: &DiscussionMessage, messages: &[DiscussionMessage]) -> String {
    let Some(reply_id) = message.reply_to_message_id.as_deref() else {
        return String::new();
    };
    let message_ref = format!("MSG-{}", reply_id.chars().take(8).collect::<String>());
    let Some(target) = messages.iter().find(|candidate| candidate.id == reply_id) else {
        return format!("[Reply to missing message {message_ref}]\n");
    };
    let author = target
        .agent_type
        .as_ref()
        .map(agent_display_name)
        .or_else(|| target.author_pseudo.clone())
        .unwrap_or_else(|| match target.role {
            MessageRole::User => "User".into(),
            MessageRole::Agent => "Agent".into(),
            MessageRole::System => "System".into(),
        });
    let normalized = target
        .content
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut excerpt = normalized.chars().take(120).collect::<String>();
    if normalized.chars().count() > 120 {
        excerpt.push('…');
    }
    format!("[Reply to {message_ref} by {author}: {excerpt}]\n")
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

    // CLI agents use kronn-internal; HTTP agents receive the compact native
    // catalogue in `agent_tools.rs`. Vibe deliberately runs without MCP
    // because its stdio integration hangs, so it can only emit a durable,
    // human-gated proposal fence. Custom is not an executable runtime.
    let agent_speaks_mcp = matches!(
        agent_type,
        AgentType::ClaudeCode
            | AgentType::Codex
            | AgentType::GeminiCli
            | AgentType::Kiro
            | AgentType::CopilotCli
    );
    let agent_has_native_planning = matches!(agent_type, AgentType::Ollama | AgentType::LiteLlm);

    // This is the compact room-rendering contract, not the document authoring
    // manual. Keep it visible to every runtime; exact payload shapes remain in
    // the Kronn Docs skill and are only loaded when needed.
    let rich_output_notice = match disc.language.as_str() {
        "fr" => "Rendu enrichi Kronn — réponds en Markdown. Si un visuel apporte réellement quelque chose, utilise un bloc `mermaid` (flowchart/graph, sequenceDiagram, classDiagram, stateDiagram, erDiagram, journey, gantt, pie, gitGraph, C4*, requirementDiagram, mindmap, timeline, sankey-beta, xychart-beta, block-beta ou packet-beta). Pour un aperçu HTML isolé avec boutons PDF/DOCX, utilise `kronn-doc-preview` — pas un bloc `html` ordinaire. Pour exporter des données CSV/XLSX/PPTX, utilise un JSON `kronn-doc-data` au format attendu par le skill Kronn Docs.\n\n",
        "es" => "Salida enriquecida de Kronn — responde en Markdown. Si un recurso visual aporta valor real, usa un bloque `mermaid` (flowchart/graph, sequenceDiagram, classDiagram, stateDiagram, erDiagram, journey, gantt, pie, gitGraph, C4*, requirementDiagram, mindmap, timeline, sankey-beta, xychart-beta, block-beta o packet-beta). Para una vista previa HTML aislada con botones PDF/DOCX, usa `kronn-doc-preview`, no un bloque `html` normal. Para exportar datos CSV/XLSX/PPTX, usa JSON `kronn-doc-data` con la forma indicada por el skill Kronn Docs.\n\n",
        "zh" => "Kronn 富文本输出 — 请使用 Markdown。仅在图示确有帮助时使用 `mermaid` 代码块（flowchart/graph、sequenceDiagram、classDiagram、stateDiagram、erDiagram、journey、gantt、pie、gitGraph、C4*、requirementDiagram、mindmap、timeline、sankey-beta、xychart-beta、block-beta 或 packet-beta）。如需带 PDF/DOCX 按钮的隔离 HTML 预览，请使用 `kronn-doc-preview`，不要使用普通 `html` 代码块。导出 CSV/XLSX/PPTX 数据时，请按 Kronn Docs skill 的格式输出 `kronn-doc-data` JSON。\n\n",
        _ => "Kronn rich output — reply in Markdown. When a visual materially helps, use a `mermaid` fence (flowchart/graph, sequenceDiagram, classDiagram, stateDiagram, erDiagram, journey, gantt, pie, gitGraph, C4*, requirementDiagram, mindmap, timeline, sankey-beta, xychart-beta, block-beta or packet-beta). For a sandboxed HTML preview with PDF/DOCX buttons, use `kronn-doc-preview`, not a normal `html` fence. For CSV/XLSX/PPTX data export, use `kronn-doc-data` JSON in the shape documented by the Kronn Docs skill.\n\n",
    };

    // Planning is useful durable context even on turn one. Keep the notice
    // compact and inject no task body: agents pull `plan_get` only when the
    // request concerns tracked work. The explicit id makes CLI writes robust
    // to stale MCP session bindings and removes the need for a join token.
    let planning_notice = match (disc.language.as_str(), agent_speaks_mcp, agent_has_native_planning)
    {
        ("fr", true, _) => format!(
            "Planification Kronn — cette discussion est `{}` et peut avoir un plan partagé composé de tâches priorisées. `plan_get()` lit ce plan. Si la demande concerne un plan ou du travail à retenir, appelle `plan_get({{discussion_id: \"{}\"}})` avant les outils `task_*`, puis passe ce même `discussion_id` à `task_create`/`task_link_discussion`. Cet identifiant explicite fonctionne même si le MCP annonce `no disc bound` ou `rejoin_required` : ne demande pas de token d'invitation. Effectue directement une demande non ambiguë ; sinon émets un bloc `kronn-plan-action` soumis à validation humaine. Ne remplace jamais une mise à jour demandée par un simple résumé Markdown.\n\n",
            disc.id, disc.id
        ),
        ("es", true, _) => format!(
            "Planificación de Kronn — esta conversación es `{}`. Si la solicitud trata de un plan o trabajo que debe conservarse, llama a `plan_get({{discussion_id: \"{}\"}})` antes de las herramientas `task_*` y pasa el mismo `discussion_id` a `task_create`. El id explícito funciona incluso si MCP indica `no disc bound` o `rejoin_required`: no pidas un token de invitación. Aplica directamente una solicitud inequívoca; si es ambigua, emite un bloque `kronn-plan-action` sujeto a validación humana. No sustituyas una actualización solicitada por un simple resumen Markdown.\n\n",
            disc.id, disc.id
        ),
        ("zh", true, _) => format!(
            "Kronn 计划 — 当前讨论 ID 为 `{}`。当请求涉及需要保留的计划或任务时，先调用 `plan_get({{discussion_id: \"{}\"}})`，再调用 `task_*` 工具，并把同一 `discussion_id` 传给 `task_create`。即使 MCP 返回 `no disc bound` 或 `rejoin_required`，显式 ID 仍然有效；不要索要邀请令牌。明确的请求可直接执行；有歧义时输出需人工确认的 `kronn-plan-action` 代码块。不要只用 Markdown 计划代替用户要求的更新。\n\n",
            disc.id, disc.id
        ),
        (_, true, _) => format!(
            "Kronn Planning — this discussion is `{}`. When the request concerns a plan or work worth retaining, call `plan_get({{discussion_id: \"{}\"}})` before `task_*` tools and pass the same `discussion_id` to `task_create`. The explicit id works even when MCP reports `no disc bound` or `rejoin_required`; do not ask for an invitation token. Apply an unambiguous request directly; otherwise emit a human-gated `kronn-plan-action` fence. Never replace a requested update with a prose-only Markdown plan.\n\n",
            disc.id, disc.id
        ),
        ("fr", _, true) => "Planification Kronn — les outils natifs `plan_get` et `task_*` sont déjà limités à cette discussion. Si la demande concerne un plan ou du travail à retenir, lis le plan avant toute modification. Effectue directement une demande non ambiguë ; sinon émets un bloc `kronn-plan-action` soumis à validation humaine. Ne remplace jamais une mise à jour demandée par un simple résumé Markdown.\n\n".into(),
        ("es", _, true) => "Planificación de Kronn — las herramientas nativas `plan_get` y `task_*` ya están limitadas a esta conversación. Si la solicitud trata de un plan o trabajo que debe conservarse, lee el plan antes de modificarlo. Aplica directamente una solicitud inequívoca; si es ambigua, emite un bloque `kronn-plan-action` sujeto a validación humana. No sustituyas una actualización solicitada por un simple plan Markdown.\n\n".into(),
        ("zh", _, true) => "Kronn 计划 — 原生 `plan_get` 和 `task_*` 工具已限定到当前讨论。当请求涉及需要保留的计划或任务时，请先读取计划再修改。明确的请求可直接执行；有歧义时输出需人工确认的 `kronn-plan-action` 代码块。不要只用 Markdown 计划代替用户要求的更新。\n\n".into(),
        (_, _, true) => "Kronn Planning — native `plan_get` and `task_*` tools are already scoped to this discussion. When the request concerns a plan or work worth retaining, read the plan before changing it. Apply an unambiguous request directly; otherwise emit a human-gated `kronn-plan-action` fence. Never replace a requested update with a prose-only Markdown plan.\n\n".into(),
        ("fr", _, _) => "Planification Kronn — ce runtime ne peut pas appeler directement les outils Planning. Si la demande concerne le plan ou les tâches, ne réponds pas uniquement en Markdown : émets un bloc `kronn-plan-action` qui sera soumis à validation humaine dans Kronn.\n\n".into(),
        ("es", _, _) => "Planificación de Kronn — este runtime no puede llamar directamente a las herramientas de Planning. Si la solicitud trata del plan o las tareas, no respondas solo con Markdown: emite un bloque `kronn-plan-action` que se someterá a validación humana en Kronn.\n\n".into(),
        ("zh", _, _) => "Kronn 计划 — 此运行时无法直接调用计划工具。如果请求涉及计划或任务，请不要只回复 Markdown；请输出 `kronn-plan-action` 代码块，由用户在 Kronn 中确认。\n\n".into(),
        (_, _, _) => "Kronn Planning — this runtime cannot call Planning tools directly. If the request concerns the plan or tasks, do not reply with Markdown alone: emit a `kronn-plan-action` fence for human validation in Kronn.\n\n".into(),
    };

    // History heads-up stays delayed: unlike the Planning capability pointer,
    // it is dead weight while the complete short thread is already in-window.
    let introspection_notice = match disc.language.as_str() {
        "fr" => "Outils d'historique disponibles via le MCP `kronn-internal` : \
            `disc_meta()` (compte de messages, agent, tier — gratuit), \
            `disc_get_message(idx | message_id, before?, after?)` (un message précis par index ou référence `MSG-…`, avec petit contexte optionnel — gratuit), \
            `disc_summarize(from?, to?)` (synthèse à la demande — coûte des tokens). \
            N'utilise CES OUTILS QUE si tu remarques un trou de contexte que tu ne peux pas déduire de la fenêtre courante. Pas en spéculation.\n\
            L'utilisateur peut parler naturellement du plan comme « le plan », « les tâches », « ce qu'il reste » ou « la priorité ». \
            `plan_get`/`task_list`/`task_get`/`task_changes` lisent le travail ; `proposal_list`/`proposal_get` lisent les propositions durables. \
            Juste avant tout `task_create` direct, rappelle `plan_get` afin de voir l'écriture récente d'un pair. Pour une intention ambiguë, émets un bloc `kronn-plan-action` (create, create_many, status, complete, unblock, open) : seul un humain accepte, refuse ou décide une proposition durable. \
            Quand un travail suivi démarre ou change réellement, maintiens son statut, sa DoD et sa priorité et ne recharge ou réécris jamais une tâche inchangée. Si les outils annoncés manquent, utilise l'instantané `plan_snapshot` de `disc_join` en lecture seule et n'invente aucune mise à jour.\n\n",
        "es" => "Herramientas de historial vía MCP `kronn-internal`: \
            `disc_meta()` (cuenta de mensajes, agente, tier — gratuito), \
            `disc_get_message(idx | message_id, before?, after?)` (un mensaje por índice o referencia `MSG-…`, con contexto pequeño opcional — gratuito), \
            `disc_summarize(from?, to?)` (resumen bajo demanda — cuesta tokens). \
            Úsalos SOLO cuando notes un hueco de contexto que no puedas deducir de la ventana actual.\n\
            El usuario puede hablar naturalmente de « el plan », « las tareas », « lo que queda » o « la prioridad ». \
            `plan_get`/`task_list`/`task_get`/`task_changes` leen el trabajo; `proposal_list`/`proposal_get` leen propuestas durables. \
            Justo antes de cualquier `task_create` directo, vuelve a llamar a `plan_get` para ver la escritura reciente de otro agente. Para una intención ambigua, emite un bloque `kronn-plan-action` (create, create_many, status, complete, unblock, open): solo un humano acepta, rechaza o decide una propuesta durable. \
            Cuando un trabajo seguido empieza o cambia realmente, mantén su estado, DoD y prioridad y nunca recargues o reescribas una tarea sin cambios. Si faltan las herramientas anunciadas, usa el `plan_snapshot` de `disc_join` en modo de solo lectura y no inventes ninguna actualización.\n\n",
        _ => "History tools available via the `kronn-internal` MCP: \
            `disc_meta()` (message count, agent, tier — free), \
            `disc_get_message(idx | message_id, before?, after?)` (one message by index or `MSG-…` reference, with an optional small context window — free), \
            `disc_summarize(from?, to?)` (on-demand summary — costs tokens). \
            Use these ONLY when you notice a context gap you cannot infer from the current window. Never speculatively.\n\
            The user may refer to Planning naturally as “the plan”, “the tasks”, “what remains” or “the priority”. \
            `plan_get`/`task_list`/`task_get`/`task_changes` read work; `proposal_list`/`proposal_get` read durable proposals. \
            Immediately before any direct `task_create`, call `plan_get` again so a peer's recent write is visible. For ambiguous intent, emit a `kronn-plan-action` fence (create, create_many, status, complete, unblock, open): only a human accepts, rejects or decides a durable proposal. \
            Whenever tracked work materially changes, keep its status, DoD and priority honest and never reload or rewrite an unchanged task. If announced tools are absent, use the read-only `plan_snapshot` from `disc_join` and never fabricate an update.\n\n",
    };

    // Slash-marker fallback for agents that don't speak MCP (Vibe,
    // Ollama). They emit `KRONN:DISC_*` lines which the post-stream
    // parser in `slash_markers.rs` resolves into System messages on
    // the next turn. Same gating threshold as the MCP notice
    // (≥ 3 user messages — short threads have the full context).
    let slash_marker_notice = match disc.language.as_str() {
        "fr" => "Outils d'historique (sans MCP) — émets sur leur propre ligne :\n\
            • `KRONN:DISC_META` — métadonnées de la discussion\n\
            • `KRONN:DISC_GET_MESSAGE <idx>` — un message précis (idx négatif = depuis la fin)\n\
            • `KRONN:DISC_SUMMARIZE <from> <to>` — synthèse d'une plage [from, to)\n\
            La réponse arrivera comme message système au tour SUIVANT. Utilise UNIQUEMENT si tu as un trou de contexte que tu ne peux pas déduire de la fenêtre courante. Pas en spéculation.\n\n",
        "es" => "Herramientas de historial (sin MCP) — emite en su propia línea:\n\
            • `KRONN:DISC_META` — metadatos\n\
            • `KRONN:DISC_GET_MESSAGE <idx>` — un mensaje (idx negativo = desde el final)\n\
            • `KRONN:DISC_SUMMARIZE <from> <to>` — resumen del rango [from, to)\n\
            La respuesta llegará como mensaje del sistema en el SIGUIENTE turno. Úsalas SOLO ante un hueco de contexto real.\n\n",
        _ => "History tools (no MCP) — emit on their own line:\n\
            • `KRONN:DISC_META` — disc metadata\n\
            • `KRONN:DISC_GET_MESSAGE <idx>` — fetch one message (negative idx = from end)\n\
            • `KRONN:DISC_SUMMARIZE <from> <to>` — summarise the [from, to) range\n\
            The answer arrives as a system message on the NEXT turn. Use ONLY when you have a real context gap. Never speculatively.\n\n",
    };

    let worktree_notice = isolated_worktree_notice(disc);

    let user_msgs: Vec<_> = disc
        .messages
        .iter()
        .filter(|m| {
            matches!(m.channel, crate::models::MessageChannel::Main)
                && matches!(m.role, MessageRole::User)
        })
        .collect();
    let latest_is_peer_agent = disc
        .messages
        .iter()
        .rev()
        .find(|m| {
            matches!(m.channel, crate::models::MessageChannel::Main)
                && !matches!(m.role, MessageRole::System)
        })
        .is_some_and(|m| matches!(m.role, MessageRole::Agent));

    // The single-user-message short form intentionally omits history. A peer
    // agent may already have answered that first user message, though: in that
    // case the native principal must see and answer the peer, not replay the
    // original human prompt.
    if user_msgs.len() <= 1 && !latest_is_peer_agent {
        let content = user_msgs
            .last()
            .map(|m| format!("{}{}", reply_context(m, &disc.messages), m.content))
            .unwrap_or_default();
        // Language instruction at end only — LLMs weight recent text more heavily,
        // and MCP context is injected via --append-system-prompt (separate from prompt).
        return format!(
            "{}{}{}{}{}\n\n{}",
            title_ctx, worktree_notice, planning_notice, rich_output_notice, content, lang_instr
        );
    }

    // Fixed overhead: header + footer (localized by discussion language)
    let prev_conv_label = match disc.language.as_str() {
        "fr" => "Conversation précédente :\n\n",
        "es" => "Conversación anterior:\n\n",
        _ => "Previous conversation:\n\n",
    };
    let footer = match (disc.language.as_str(), latest_is_peer_agent) {
        ("fr", true) => "Réponds au dernier message de l'agent ci-dessus. Reponds en francais.",
        ("es", true) => "Responda al último mensaje del agente anterior. Responda en español.",
        ("zh", true) => "请回复上面的最新代理消息。请用中文回答。",
        ("br", true) => "Respont d'ar c'hemenn diwezhañ eus an agent a-us. Respont e brezhoneg.",
        ("fr", false) => "Répondez au dernier message ci-dessus. Reponds en francais.",
        ("es", false) => "Responda al último mensaje anterior. Responda en español.",
        ("zh", false) => "请回复上面的最新用户消息。请用中文回答。",
        ("br", false) => "Respontet d'ar c'hemenn diwezhañ a-us. Respont e brezhoneg.",
        (_, true) => "Please respond to the latest agent message above. Respond in English.",
        (_, false) => "Please respond to the latest user message above. Respond in English.",
    };
    // For agents that think they're in non-interactive mode (Gemini -p, Codex exec),
    // clarify that this IS a multi-turn conversation managed by Kronn.
    // Always include for pinned discussions (briefing/validation/bootstrap) since
    // agents like Gemini detect -p mode and refuse to interact on the first message.
    let interactive_hint = if user_msgs.len() > 1 || disc.pin_first_message {
        match disc.language.as_str() {
            "fr" => "NOTE: Tu es dans une conversation multi-tours geree par Kronn. Tu PEUX poser des questions et attendre des reponses. Chaque tour inclut la fenetre d'historique disponible ; si les outils d'historique sont indiques plus haut, utilise-les seulement lorsqu'un contexte plus ancien te manque.\n\n",
            "es" => "NOTA: Estas en una conversacion multi-turno gestionada por Kronn. PUEDES hacer preguntas y esperar respuestas. Cada turno incluye la ventana de historial disponible; si las herramientas de historial aparecen arriba, úsalas solo cuando te falte contexto anterior.\n\n",
            _ => "NOTE: You are in a multi-turn conversation managed by Kronn. You CAN ask questions and wait for answers. Each turn includes the available history window; when history tools are listed above, use them only when older context is missing.\n\n",
        }
    } else {
        ""
    };

    // Only include the introspection notice on threads with at least
    // 3 user messages — shorter threads have the full transcript in
    // window and the notice is just dead weight. The exact notice
    // depends on whether the agent speaks MCP:
    //   - MCP-speakers: get the `kronn-internal` tool list.
    //   - Vibe + Ollama: get the slash-marker fallback (post-stream
    //     parser in `slash_markers.rs` resolves them into System
    //     messages on the next turn).
    //   - Codex is an MCP speaker again since 0.8.6 / Codex 0.132.
    let agent_uses_slash_markers = matches!(agent_type, AgentType::Vibe | AgentType::Ollama);
    let intro_block: &str = if user_msgs.len() >= 3 {
        if agent_speaks_mcp {
            introspection_notice
        } else if agent_uses_slash_markers {
            slash_marker_notice
        } else {
            ""
        }
    } else {
        ""
    };
    let header = format!(
        "{}{}{}{}{}{}{}",
        title_ctx,
        worktree_notice,
        planning_notice,
        rich_output_notice,
        intro_block,
        interactive_hint,
        prev_conv_label
    );
    let overhead = header.len() + footer.len() + 100; // 100 = notice template space

    // If pin_first_message is set, extract and pin the first non-system message
    let non_system_msgs: Vec<_> = disc
        .messages
        .iter()
        .filter(|m| {
            matches!(m.channel, crate::models::MessageChannel::Main)
                && !matches!(m.role, MessageRole::System)
        })
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
            "fr" => format!(
                "Résumé de la conversation précédente (messages 1-{}) :\n{}\n\n",
                idx, summary
            ),
            "es" => format!(
                "Resumen de la conversación anterior (mensajes 1-{}):\n{}\n\n",
                idx, summary
            ),
            _ => format!(
                "Summary of earlier conversation (messages 1-{}):\n{}\n\n",
                idx, summary
            ),
        }
    } else {
        String::new()
    };

    let remaining_budget =
        budget.saturating_sub(overhead + pinned_block.len() + summary_block.len());

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
            MessageRole::User => format!(
                "User: {}{}\n\n",
                reply_context(msg, &disc.messages),
                msg.content
            ),
            MessageRole::Agent => {
                let agent_label = msg
                    .agent_type
                    .as_ref()
                    .map(agent_display_name)
                    .unwrap_or_else(|| "Agent".into());
                format!(
                    "{}: {}{}\n\n",
                    agent_label,
                    reply_context(msg, &disc.messages),
                    msg.content
                )
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
                if has_summary {
                    " (resume ci-dessus)"
                } else {
                    " — demandez a l'utilisateur si besoin"
                }
            ),
            "es" => format!(
                "════════════════════════════════════════\n\
                 CONTEXTO LIMITADO: {} mensajes anteriores no incluidos{}\n\
                 ════════════════════════════════════════\n\n",
                omitted_count,
                if has_summary {
                    " (resumen arriba)"
                } else {
                    " — pregunte al usuario si necesario"
                }
            ),
            _ => format!(
                "════════════════════════════════════════\n\
                 CONTEXT LIMITED: {} earlier messages not included{}\n\
                 ════════════════════════════════════════\n\n",
                omitted_count,
                if has_summary {
                    " (see summary above)"
                } else {
                    " — ask user to recap if needed"
                }
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
            awaiting_agent: false,
            id: "d-test".into(),
            project_id: None,
            title: "Test discussion".into(),
            agent: AgentType::ClaudeCode,
            language: language.into(),
            participants: vec![],
            messages,
            message_count: 0,
            non_system_message_count: 0,
            skill_ids: vec![],
            profile_ids: vec![],
            directive_ids: vec![],
            archived: false,
            pinned: false,
            workspace_mode: "Direct".into(),
            workspace_path: None,
            worktree_branch: None,
            tier: ModelTier::Default,
            model: None,
            pin_first_message: false,
            summary_cache: None,
            summary_up_to_msg_idx: None,
            summary_strategy: crate::models::SummaryStrategy::Auto,
            introspection_call_count: 0,
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
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::User,
            channel: crate::models::MessageChannel::Main,
            content: content.into(),
            agent_type: None,
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: None,
            duration_ms: None,
            target_agent: None,
            reply_to_message_id: None,
        }
    }

    fn agent_msg(content: &str, agent_type: AgentType) -> DiscussionMessage {
        DiscussionMessage {
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
            id: uuid::Uuid::new_v4().to_string(),
            role: MessageRole::Agent,
            channel: crate::models::MessageChannel::Main,
            content: content.into(),
            agent_type: Some(agent_type),
            timestamp: chrono::Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: Some("peer-turn".into()),
            duration_ms: None,
            target_agent: None,
            reply_to_message_id: None,
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
    fn first_turn_exposes_planning_with_an_explicit_discussion_id() {
        let disc = disc_with_messages(
            vec![user_msg("Create the discussion plan with one task per day")],
            "en",
        );
        let prompt = build_agent_prompt(&disc, &AgentType::Codex, 0);

        assert!(prompt.contains("Kronn Planning"));
        assert!(prompt.contains(&disc.id));
        assert!(prompt.contains("no disc bound"));
        assert!(prompt.contains("do not ask for an invitation token"));
        assert!(prompt.contains("Never replace a requested update"));
        assert!(
            !prompt.contains("History tools available"),
            "history discovery remains delayed; only the compact Planning capability is turn-one context"
        );
    }

    #[test]
    fn first_turn_http_agent_gets_native_planning_instructions() {
        for agent in [AgentType::Ollama, AgentType::LiteLlm] {
            let disc = disc_with_messages(vec![user_msg("Track this in the plan")], "en");
            let prompt = build_agent_prompt(&disc, &agent, 0);

            assert!(prompt.contains("native `plan_get` and `task_*` tools"));
            assert!(prompt.contains("scoped to this discussion"));
            assert!(!prompt.contains("no disc bound"));
        }
    }

    #[test]
    fn every_discussion_agent_knows_the_rich_output_fences() {
        for agent in [
            AgentType::ClaudeCode,
            AgentType::Codex,
            AgentType::Vibe,
            AgentType::GeminiCli,
            AgentType::Kiro,
            AgentType::CopilotCli,
            AgentType::Ollama,
            AgentType::LiteLlm,
            AgentType::Custom,
        ] {
            let disc = disc_with_messages(vec![user_msg("Show the architecture")], "en");
            let prompt = build_agent_prompt(&disc, &agent, 0);

            for contract in [
                "`mermaid`",
                "sequenceDiagram",
                "C4*",
                "packet-beta",
                "`kronn-doc-preview`",
                "not a normal `html` fence",
                "PDF/DOCX",
                "`kronn-doc-data`",
                "CSV/XLSX/PPTX",
            ] {
                assert!(
                    prompt.contains(contract),
                    "{agent:?} prompt must expose rich-output contract {contract}: {prompt}"
                );
            }
        }
    }

    #[test]
    fn first_turn_vibe_gets_a_human_gated_planning_fallback() {
        let disc = disc_with_messages(vec![user_msg("Track this in the plan")], "en");
        let prompt = build_agent_prompt(&disc, &AgentType::Vibe, 0);

        assert!(prompt.contains("cannot call Planning tools directly"));
        assert!(prompt.contains("`kronn-plan-action`"));
        assert!(prompt.contains("human validation"));
    }

    #[test]
    fn agent_prompt_multi_message_includes_history_and_footer() {
        let msgs = vec![user_msg("first question"), user_msg("follow-up question")];
        let disc = disc_with_messages(msgs, "en");
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("Previous conversation"));
        assert!(prompt.contains("first question"));
        assert!(prompt.contains("follow-up question"));
        assert!(prompt.contains("Please respond to the latest user message"));
    }

    #[test]
    fn agent_prompt_excludes_out_of_context_notes() {
        let mut note = user_msg("private note body");
        note.channel = crate::models::MessageChannel::Note;
        let disc = disc_with_messages(vec![user_msg("visible question"), note], "en");

        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);

        assert!(prompt.contains("visible question"));
        assert!(!prompt.contains("private note body"));
    }

    #[test]
    fn agent_prompt_exposes_durable_reply_context() {
        let original = agent_msg(
            "The migration needs a compatibility test.",
            AgentType::ClaudeCode,
        );
        let mut reply = user_msg("I will add it.");
        reply.reply_to_message_id = Some(original.id.clone());
        let expected_ref = format!("MSG-{}", original.id.chars().take(8).collect::<String>());
        let disc = disc_with_messages(vec![original, reply], "en");

        let prompt = build_agent_prompt(&disc, &AgentType::Codex, 0);

        assert!(prompt.contains(&format!(
            "[Reply to {expected_ref} by Claude Code: The migration needs a compatibility test.]"
        )));
        assert!(prompt.contains("I will add it."));
    }

    #[test]
    fn agent_prompt_keeps_missing_reply_reference_visible() {
        let mut reply = user_msg("The source message was not imported.");
        reply.reply_to_message_id = Some("11111111-2222-3333-4444-555555555555".into());
        let disc = disc_with_messages(vec![reply], "en");

        let prompt = build_agent_prompt(&disc, &AgentType::Codex, 0);

        assert!(prompt.contains("[Reply to missing message MSG-11111111]"));
    }

    #[test]
    fn agent_prompt_answers_a_peer_even_after_only_one_human_message() {
        let msgs = vec![
            user_msg("Can one of you investigate?"),
            agent_msg(
                "I found the failing route; can Codex verify it?",
                AgentType::ClaudeCode,
            ),
        ];
        let disc = disc_with_messages(msgs, "en");
        let prompt = build_agent_prompt(&disc, &AgentType::Codex, 0);

        assert!(prompt.contains("Can one of you investigate?"));
        assert!(prompt.contains("Claude Code: I found the failing route"));
        assert!(prompt.contains("Please respond to the latest agent message"));
    }

    #[test]
    fn agent_prompt_maps_discussion_plan_wording_to_structured_mcp_tools() {
        let disc = disc_with_messages(
            vec![
                user_msg("premier tour"),
                user_msg("deuxième tour"),
                user_msg("mets à jour le plan de discussion"),
            ],
            "fr",
        );
        let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);
        assert!(prompt.contains("plan partagé composé de tâches priorisées"));
        assert!(prompt.contains("« le plan », « les tâches »"));
        assert!(prompt.contains("simple résumé Markdown"));
        assert!(prompt.contains("`plan_get()`"));
        assert!(prompt.contains("`task_link_discussion`"));
    }

    /// Single source of truth for the Planning contract, shared with the MCP
    /// bridge (`backend/scripts/planning_contract_invariants.json`). Parsed at
    /// compile time so a missing/renamed file breaks the build, not silently
    /// the test.
    fn planning_contract() -> serde_json::Value {
        let raw = include_str!("../../scripts/planning_contract_invariants.json");
        serde_json::from_str(raw).expect("planning_contract_invariants.json must be valid JSON")
    }

    fn contract_strings(v: &serde_json::Value, key: &str) -> Vec<String> {
        v[key]
            .as_array()
            .unwrap_or_else(|| panic!("`{key}` must be a JSON array"))
            .iter()
            .map(|s| {
                s.as_str()
                    .expect("array entries must be strings")
                    .to_string()
            })
            .collect()
    }

    /// KT-29 (0.9.2-I) parity: the Kronn-launched agent prompt notice must
    /// carry every Planning-contract invariant, in each supported locale. The
    /// bridge side (`test_disc_introspection_mcp.py`) checks the SAME JSON
    /// against the MCP `instructions` block, so neither surface can drift the
    /// contract without failing its own test — no fragile textual equality.
    #[test]
    fn planning_contract_parity_kronn_launched_prompt_notice() {
        let contract = planning_contract();
        let fence = contract["fence"].as_str().unwrap();
        let read_tools = contract_strings(&contract, "read_tools");
        let actions = contract_strings(&contract, "proposal_actions");

        for lang in ["en", "fr", "es"] {
            // ≥3 user messages so the introspection notice is injected.
            let disc = disc_with_messages(
                vec![user_msg("un"), user_msg("deux"), user_msg("trois")],
                lang,
            );
            let prompt = build_agent_prompt(&disc, &AgentType::ClaudeCode, 0);

            assert!(
                prompt.contains(fence),
                "[{lang}] prompt must name the `{fence}` fence"
            );
            for tool in &read_tools {
                assert!(
                    prompt.contains(tool),
                    "[{lang}] prompt must mention the read tool `{tool}`"
                );
            }
            for action in &actions {
                assert!(
                    prompt.contains(action),
                    "[{lang}] prompt must list the proposal action `{action}`"
                );
            }
            for alias in contract["aliases"][lang]
                .as_array()
                .unwrap_or_else(|| panic!("aliases.{lang} must be a JSON array"))
            {
                let alias = alias.as_str().unwrap();
                assert!(
                    prompt.contains(alias),
                    "[{lang}] prompt must recognise the human alias `{alias}`"
                );
            }
            let no_prose = contract["no_prose_only"][lang].as_str().unwrap();
            assert!(
                prompt.contains(no_prose),
                "[{lang}] prompt must forbid a prose-only reply (marker `{no_prose}`)"
            );
            let human_decides = contract["human_decides"][lang].as_str().unwrap();
            assert!(
                prompt.contains(human_decides),
                "[{lang}] prompt must state that only a human decides a durable proposal (marker `{human_decides}`)"
            );
            for invariant in [
                "maintain_on_change",
                "no_noop_writes",
                "read_before_direct_create",
                "stale_surface_fallback",
            ] {
                let marker = contract[invariant][lang]
                    .as_str()
                    .unwrap_or_else(|| panic!("{invariant}.{lang} must be a string"));
                assert!(
                    prompt.contains(marker),
                    "[{lang}] prompt must carry `{invariant}` (marker `{marker}`)"
                );
            }
        }
    }

    /// Vibe still uses the one-turn-late history marker fallback. Ollama has
    /// native Planning tools but keeps markers for history introspection.
    #[test]
    fn history_marker_fallback_remains_available_to_vibe_and_ollama() {
        for agent in [AgentType::Vibe, AgentType::Ollama] {
            let disc = disc_with_messages(
                vec![user_msg("one"), user_msg("two"), user_msg("three")],
                "en",
            );
            let prompt = build_agent_prompt(&disc, &agent, 0);
            assert!(
                prompt.contains("KRONN:DISC_META"),
                "{agent:?} must receive the slash-marker fallback instead"
            );
        }
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
        assert!(
            prompt.contains("GIT ISOLATION"),
            "notice heading missing: {}",
            prompt
        );
        assert!(
            prompt.contains("kronn/add-feature"),
            "branch name missing: {}",
            prompt
        );
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
        assert!(
            notice_pos < conv_pos,
            "worktree notice should precede conversation"
        );
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
        let r2 = vec![
            ("A".into(), "final-a".into()),
            ("B".into(), "final-b".into()),
        ];
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
