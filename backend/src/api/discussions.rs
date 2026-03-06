use std::convert::Infallible;
use std::pin::Pin;
use axum::{
    extract::{Path, State},
    response::sse::{Event, Sse},
    Json,
};
use chrono::Utc;
use futures::stream::Stream;
use uuid::Uuid;

use crate::agents::runner;
use crate::models::*;
use crate::AppState;

/// GET /api/discussions
pub async fn list(State(state): State<AppState>) -> Json<ApiResponse<Vec<Discussion>>> {
    match state.db.with_conn(|conn| crate::db::discussions::list_discussions(conn)).await {
        Ok(discussions) => Json(ApiResponse::ok(discussions)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// GET /api/discussions/:id
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<Discussion>> {
    match state.db.with_conn(move |conn| crate::db::discussions::get_discussion(conn, &id)).await {
        Ok(Some(d)) => Json(ApiResponse::ok(d)),
        Ok(None) => Json(ApiResponse::err("Discussion not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// POST /api/discussions
pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateDiscussionRequest>,
) -> Json<ApiResponse<Discussion>> {
    // Validate project exists (if specified)
    if let Some(ref pid) = req.project_id {
        let pid = pid.clone();
        let project_exists = state.db.with_conn({
            move |conn| {
                let p = crate::db::projects::get_project(conn, &pid)?;
                Ok(p.is_some())
            }
        }).await.unwrap_or(false);

        if !project_exists {
            return Json(ApiResponse::err("Project not found"));
        }
    }

    let language = if req.language.is_empty() {
        let config = state.config.read().await;
        config.language.clone()
    } else {
        req.language
    };

    let now = Utc::now();
    let initial_message = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.initial_prompt,
        agent_type: None,
        timestamp: now,
    };

    let discussion = Discussion {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        title: req.title,
        agent: req.agent.clone(),
        language,
        participants: vec![req.agent.clone()],
        messages: vec![initial_message.clone()],
        created_at: now,
        updated_at: now,
    };

    let disc = discussion.clone();
    let msg = initial_message;
    match state.db.with_conn(move |conn| {
        crate::db::discussions::insert_discussion(conn, &disc)?;
        crate::db::discussions::insert_message(conn, &disc.id, &msg)?;
        Ok(())
    }).await {
        Ok(()) => Json(ApiResponse::ok(discussion)),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/discussions/:id
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::discussions::delete_discussion(conn, &id)).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Discussion not found")),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// DELETE /api/discussions/:id/messages/last
pub async fn delete_last_agent_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<ApiResponse<()>> {
    match state.db.with_conn(move |conn| crate::db::discussions::delete_last_agent_messages(conn, &id)).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

/// PATCH /api/discussions/:id/messages/last
pub async fn edit_last_user_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Json<ApiResponse<()>> {
    let content = req.content;
    match state.db.with_conn(move |conn| crate::db::discussions::edit_last_user_message(conn, &id, &content)).await {
        Ok(_) => Json(ApiResponse::ok(())),
        Err(e) => Json(ApiResponse::err(format!("DB error: {}", e))),
    }
}

type SseStream = Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

/// POST /api/discussions/:id/messages
pub async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> Sse<SseStream> {
    let target = req.target_agent.clone();

    // Add user message to DB
    let user_msg = DiscussionMessage {
        id: Uuid::new_v4().to_string(),
        role: MessageRole::User,
        content: req.content.clone(),
        agent_type: None,
        timestamp: Utc::now(),
    };
    let disc_id = id.clone();
    let msg = user_msg.clone();
    let target_clone = target.clone();
    let _ = state.db.with_conn(move |conn| {
        crate::db::discussions::insert_message(conn, &disc_id, &msg)?;
        // Track new participant
        if let Some(ref t) = target_clone {
            let disc = crate::db::discussions::get_discussion(conn, &disc_id)?;
            if let Some(d) = disc {
                if !d.participants.contains(t) {
                    let mut participants = d.participants;
                    participants.push(t.clone());
                    crate::db::discussions::update_discussion_participants(conn, &disc_id, &participants)?;
                }
            }
        }
        Ok(())
    }).await;

    make_agent_stream(state, id, target).await
}

/// POST /api/discussions/:id/run
pub async fn run_agent(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Sse<SseStream> {
    make_agent_stream(state, id, None).await
}

/// Shared SSE stream builder
async fn make_agent_stream(
    state: AppState,
    discussion_id: String,
    agent_override: Option<AgentType>,
) -> Sse<SseStream> {
    // Extract info from DB
    let disc = state.db.with_conn({
        let did = discussion_id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await.ok().flatten();

    if disc.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(
                Event::default().event("error").data("{\"error\":\"Discussion not found\"}")
            )
        }));
        return Sse::new(stream);
    }

    let disc = disc.unwrap();
    let agent_type = agent_override.unwrap_or_else(|| disc.agent.clone());
    let prompt = build_agent_prompt(&disc);

    let project_path = if let Some(ref pid) = disc.project_id {
        let pid = pid.clone();
        state.db.with_conn(move |conn| {
            let p = crate::db::projects::get_project(conn, &pid)?;
            Ok(p.map(|p| p.path).unwrap_or_default())
        }).await.unwrap_or_default()
    } else {
        String::new()
    };

    let (tokens, full_access) = {
        let config = state.config.read().await;
        let fa = match agent_type {
            AgentType::ClaudeCode => config.agents.claude_code.full_access,
            AgentType::Codex => config.agents.codex.full_access,
            _ => false,
        };
        (config.tokens.clone(), fa)
    };

    let state_clone = state.clone();
    let disc_id = discussion_id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        yield Event::default().event("start").data("{}");

        match runner::start_agent(&agent_type, &project_path, &prompt, &tokens, full_access).await {
            Ok(mut process) => {
                let mut full_response = String::new();

                while let Some(line) = process.next_line().await {
                    if !full_response.is_empty() {
                        full_response.push('\n');
                    }
                    full_response.push_str(&line);

                    let chunk = serde_json::json!({ "text": line });
                    yield Event::default().event("chunk").data(chunk.to_string());
                }

                let status = process.child.wait().await;
                let success = status.map(|s| s.success()).unwrap_or(false);

                // Detect authentication errors and add helpful guidance
                if !success || full_response.contains("authentication_error") || full_response.contains("Invalid authentication") {
                    let is_auth_error = full_response.contains("authentication_error")
                        || full_response.contains("Invalid authentication")
                        || full_response.contains("API Error: 401");

                    if is_auth_error {
                        full_response.push_str("\n\n⚠️ Session expirée. Reconnectez-vous en lançant `/login` dans le CLI de l'agent concerné.");
                    }
                }

                if full_response.is_empty() && !success {
                    // Show captured stderr when agent fails silently
                    let stderr_lines = process.captured_stderr();
                    if stderr_lines.is_empty() {
                        full_response = "[Agent exited with error]".to_string();
                    } else {
                        full_response = format!("[Agent exited with error]\n\n{}", stderr_lines.join("\n"));
                    }
                }

                // Save agent response to DB
                let agent_msg = DiscussionMessage {
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::Agent,
                    content: full_response,
                    agent_type: Some(agent_type.clone()),
                    timestamp: Utc::now(),
                };

                let did = disc_id.clone();
                let msg = agent_msg.clone();
                let _ = state_clone.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &msg)
                }).await;

                let done = serde_json::json!({ "message_id": agent_msg.id, "success": success });
                yield Event::default().event("done").data(done.to_string());
            }
            Err(e) => {
                tracing::error!("Agent start failed: {}", e);

                let err_msg = DiscussionMessage {
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::System,
                    content: format!("Erreur: {}", e),
                    agent_type: None,
                    timestamp: Utc::now(),
                };

                let did = disc_id.clone();
                let _ = state_clone.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &err_msg)
                }).await;

                let err = serde_json::json!({ "error": e });
                yield Event::default().event("error").data(err.to_string());
            }
        }
    });

    Sse::new(stream)
}

/// POST /api/discussions/:id/orchestrate
pub async fn orchestrate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<OrchestrationRequest>,
) -> Sse<SseStream> {
    let agents = req.agents;
    let max_rounds = req.max_rounds.unwrap_or(3).min(3);

    if agents.len() < 2 {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().event("error").data("{\"error\":\"At least 2 agents required\"}"))
        }));
        return Sse::new(stream);
    }

    // Extract discussion info from DB
    let disc = state.db.with_conn({
        let did = id.clone();
        move |conn| crate::db::discussions::get_discussion(conn, &did)
    }).await.ok().flatten();

    if disc.is_none() {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().event("error").data("{\"error\":\"Discussion not found\"}"))
        }));
        return Sse::new(stream);
    }

    let disc = disc.unwrap();
    let original_question = disc.messages.iter().rev()
        .find(|m| matches!(m.role, MessageRole::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let disc_language = disc.language.clone();
    let primary_agent_type = disc.agent.clone();

    // Reorder agents: non-primary first, primary last
    let agents = {
        let mut others: Vec<_> = agents.iter().filter(|a| **a != primary_agent_type).cloned().collect();
        others.push(primary_agent_type.clone());
        others
    };

    let project_path = if let Some(ref pid) = disc.project_id {
        let pid = pid.clone();
        state.db.with_conn(move |conn| {
            let p = crate::db::projects::get_project(conn, &pid)?;
            Ok(p.map(|p| p.path).unwrap_or_default())
        }).await.unwrap_or_default()
    } else {
        String::new()
    };

    let (tokens, agent_access) = {
        let config = state.config.read().await;
        let access = |a: &AgentType| match a {
            AgentType::ClaudeCode => config.agents.claude_code.full_access,
            AgentType::Codex => config.agents.codex.full_access,
            _ => false,
        };
        let access_map: std::collections::HashMap<String, bool> = agents.iter()
            .map(|a| (format!("{:?}", a), access(a)))
            .collect();
        (config.tokens.clone(), access_map)
    };

    // Update participants
    {
        let did = id.clone();
        let all_agents = agents.clone();
        let mut participants = disc.participants.clone();
        for a in &all_agents {
            if !participants.contains(a) {
                participants.push(a.clone());
            }
        }
        let _ = state.db.with_conn(move |conn| {
            crate::db::discussions::update_discussion_participants(conn, &did, &participants)
        }).await;
    }

    let state_clone = state.clone();
    let disc_id = id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        let agent_names: Vec<String> = agents.iter().map(agent_display_name).collect();
        let sys_text = format!(
            "Mode orchestration active avec {}. Les agents vont debattre sur {} rounds maximum.",
            agent_names.join(", "), max_rounds
        );
        yield Event::default().event("system").data(
            serde_json::json!({ "text": sys_text, "agents": agent_names }).to_string()
        );

        // Save system message
        {
            let msg = DiscussionMessage {
                id: Uuid::new_v4().to_string(),
                role: MessageRole::System,
                content: sys_text.clone(),
                agent_type: None,
                timestamp: Utc::now(),
            };
            let did = disc_id.clone();
            let _ = state_clone.db.with_conn(move |conn| {
                crate::db::discussions::insert_message(conn, &did, &msg)
            }).await;
        }

        let mut round_responses: Vec<Vec<(String, String)>> = Vec::new();

        for round in 1..=max_rounds {
            yield Event::default().event("round").data(
                serde_json::json!({ "round": round, "total": max_rounds }).to_string()
            );

            let mut this_round: Vec<(String, String)> = Vec::new();

            for agent_type in &agents {
                let agent_name = agent_display_name(agent_type);

                yield Event::default().event("agent_start").data(
                    serde_json::json!({ "agent": agent_name, "agent_type": agent_type, "round": round }).to_string()
                );

                let prompt = build_orchestration_prompt(
                    &original_question, agent_type, &agent_names, &round_responses, round, max_rounds, &disc_language,
                );

                let fa = *agent_access.get(&format!("{:?}", agent_type)).unwrap_or(&false);
                match runner::start_agent(agent_type, &project_path, &prompt, &tokens, fa).await {
                    Ok(mut process) => {
                        let mut full_response = String::new();

                        while let Some(line) = process.next_line().await {
                            if !full_response.is_empty() { full_response.push('\n'); }
                            full_response.push_str(&line);

                            let chunk = serde_json::json!({
                                "text": line, "agent": agent_name,
                                "agent_type": agent_type, "round": round,
                            });
                            yield Event::default().event("chunk").data(chunk.to_string());
                        }

                        let status = process.child.wait().await;
                        let orch_success = status.map(|s| s.success()).unwrap_or(false);

                        if full_response.is_empty() && !orch_success {
                            let stderr_lines = process.captured_stderr();
                            if stderr_lines.is_empty() {
                                full_response = "[Agent exited with error]".to_string();
                            } else {
                                full_response = format!("[Agent exited with error]\n\n{}", stderr_lines.join("\n"));
                            }
                        } else if full_response.is_empty() {
                            full_response = "[No response]".to_string();
                        }

                        // Save to DB
                        {
                            let msg = DiscussionMessage {
                                id: Uuid::new_v4().to_string(),
                                role: MessageRole::Agent,
                                content: full_response.clone(),
                                agent_type: Some(agent_type.clone()),
                                timestamp: Utc::now(),
                            };
                            let did = disc_id.clone();
                            let _ = state_clone.db.with_conn(move |conn| {
                                crate::db::discussions::insert_message(conn, &did, &msg)
                            }).await;
                        }

                        yield Event::default().event("agent_done").data(
                            serde_json::json!({
                                "agent": agent_name, "agent_type": agent_type, "round": round,
                            }).to_string()
                        );

                        this_round.push((agent_name.clone(), full_response));
                    }
                    Err(e) => {
                        tracing::error!("Orchestration: agent {} failed: {}", agent_name, e);
                        let err_text = format!("[Erreur: {}]", e);
                        this_round.push((agent_name.clone(), err_text));

                        yield Event::default().event("agent_done").data(
                            serde_json::json!({
                                "agent": agent_name, "agent_type": agent_type,
                                "round": round, "error": e,
                            }).to_string()
                        );
                    }
                }
            }

            round_responses.push(this_round);

            if round >= 2 {
                yield Event::default().event("system").data(
                    serde_json::json!({ "text": format!("Round {} termine. Analyse de la convergence...", round) }).to_string()
                );
            }
        }

        // Final synthesis
        {
            let primary_name = agent_display_name(&primary_agent_type);

            yield Event::default().event("system").data(
                serde_json::json!({ "text": format!("{} synthetise le debat...", primary_name) }).to_string()
            );

            yield Event::default().event("agent_start").data(
                serde_json::json!({ "agent": primary_name, "agent_type": primary_agent_type, "round": "synthesis" }).to_string()
            );

            let synth_prompt = build_synthesis_prompt(&original_question, &round_responses, &disc_language);
            let synth_fa = *agent_access.get(&format!("{:?}", primary_agent_type)).unwrap_or(&false);
            match runner::start_agent(&primary_agent_type, &project_path, &synth_prompt, &tokens, synth_fa).await {
                Ok(mut process) => {
                    let mut full_response = String::new();
                    while let Some(line) = process.next_line().await {
                        if !full_response.is_empty() { full_response.push('\n'); }
                        full_response.push_str(&line);
                        let chunk = serde_json::json!({
                            "text": line, "agent": primary_name,
                            "agent_type": primary_agent_type, "round": "synthesis",
                        });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                    let _ = process.child.wait().await;

                    // Save synthesis to DB
                    {
                        let msg = DiscussionMessage {
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::Agent,
                            content: format!("[Synthese]\n\n{}", full_response),
                            agent_type: Some(primary_agent_type.clone()),
                            timestamp: Utc::now(),
                        };
                        let did = disc_id.clone();
                        let _ = state_clone.db.with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did, &msg)
                        }).await;
                    }

                    yield Event::default().event("agent_done").data(
                        serde_json::json!({ "agent": primary_name, "round": "synthesis" }).to_string()
                    );
                }
                Err(e) => {
                    tracing::error!("Synthesis failed: {}", e);
                    yield Event::default().event("error").data(
                        serde_json::json!({ "error": format!("Synthesis failed: {}", e) }).to_string()
                    );
                }
            }
        }

        yield Event::default().event("done").data(
            serde_json::json!({ "status": "complete" }).to_string()
        );
    });

    Sse::new(stream)
}

fn agent_display_name(agent_type: &AgentType) -> String {
    match agent_type {
        AgentType::ClaudeCode => "Claude Code".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::Custom => "Custom".into(),
    }
}

fn build_orchestration_prompt(
    question: &str,
    current_agent: &AgentType,
    all_agents: &[String],
    previous_rounds: &[Vec<(String, String)>],
    round: u32,
    max_rounds: u32,
    lang: &str,
) -> String {
    let agent_name = agent_display_name(current_agent);
    let lang_instr = language_instruction(lang);

    if round == 1 {
        format!(
            "You are {} in a technical debate between AI agents ({}).\n\
            Give your unique perspective on the question below.\n\
            Be concise and precise (max 200 words). Do NOT repeat the question.\n\
            Focus on your specific expertise and what you uniquely bring.\n\
            {}\n\n\
            Question: {}",
            agent_name, all_agents.join(", "), lang_instr, question
        )
    } else {
        let mut ctx = format!(
            "You are {} in round {}/{} of a technical debate ({}).\n\
            Here are the previous exchanges:\n\n",
            agent_name, round, max_rounds, all_agents.join(", ")
        );

        for (r_idx, round_data) in previous_rounds.iter().enumerate() {
            ctx.push_str(&format!("--- Round {} ---\n", r_idx + 1));
            for (name, response) in round_data {
                let truncated = if response.len() > 500 {
                    format!("{}...", &response[..500])
                } else {
                    response.clone()
                };
                ctx.push_str(&format!("{}: {}\n\n", name, truncated));
            }
        }

        ctx.push_str(&format!(
            "Original question: {}\n\n\
            IMPORTANT RULES:\n\
            - Do NOT repeat what others said. Do NOT summarize previous rounds.\n\
            - Only speak if you have something NEW to add: a disagreement, a nuance, a correction.\n\
            - If you agree with everything said, just state: \"I agree with the consensus.\" and stop.\n\
            - If this is round {}/{}, give your FINAL position in 1-2 sentences.\n\
            - Max 150 words.\n\
            {}",
            question, round, max_rounds, lang_instr
        ));
        ctx
    }
}

fn build_synthesis_prompt(
    question: &str,
    all_rounds: &[Vec<(String, String)>],
    lang: &str,
) -> String {
    let lang_instr = language_instruction(lang);
    let mut ctx = format!(
        "You are synthesizing a technical debate between AI agents.\n\n\
        Question: {}\n\n",
        question
    );

    if let Some(first) = all_rounds.first() {
        ctx.push_str("--- Initial positions ---\n");
        for (name, response) in first {
            let truncated = if response.len() > 400 {
                format!("{}...", &response[..400])
            } else {
                response.clone()
            };
            ctx.push_str(&format!("{}: {}\n\n", name, truncated));
        }
    }
    if all_rounds.len() > 1 {
        if let Some(last) = all_rounds.last() {
            ctx.push_str(&format!("--- Final positions (round {}) ---\n", all_rounds.len()));
            for (name, response) in last {
                let truncated = if response.len() > 400 {
                    format!("{}...", &response[..400])
                } else {
                    response.clone()
                };
                ctx.push_str(&format!("{}: {}\n\n", name, truncated));
            }
        }
    }

    ctx.push_str(&format!(
        "Produce a clear, actionable synthesis:\n\
        1. Points of AGREEMENT (what all agents converge on)\n\
        2. Remaining DISAGREEMENTS (if any)\n\
        3. FINAL RECOMMENDATION\n\
        Be concise and structured. {}", lang_instr
    ));
    ctx
}

fn language_instruction(lang: &str) -> &'static str {
    match lang {
        "fr" => "Reponds en francais.",
        "en" => "Respond in English.",
        "zh" => "请用中文回答。",
        "br" => "Respont e brezhoneg.",
        _ => "Respond in English.",
    }
}

fn build_agent_prompt(disc: &Discussion) -> String {
    let lang_instr = language_instruction(&disc.language);

    let user_msgs: Vec<_> = disc.messages.iter()
        .filter(|m| matches!(m.role, MessageRole::User))
        .collect();

    if user_msgs.len() <= 1 {
        let content = user_msgs.last().map(|m| m.content.clone()).unwrap_or_default();
        return format!("{}\n\n{}", lang_instr, content);
    }

    let mut prompt = format!("{}\n\nPrevious conversation:\n\n", lang_instr);
    for msg in &disc.messages {
        match msg.role {
            MessageRole::User => prompt.push_str(&format!("User: {}\n\n", msg.content)),
            MessageRole::Agent => {
                let agent_label = msg.agent_type.as_ref()
                    .map(|t| agent_display_name(t))
                    .unwrap_or_else(|| "Agent".into());
                prompt.push_str(&format!("{}: {}\n\n", agent_label, msg.content));
            }
            MessageRole::System => {}
        }
    }
    prompt.push_str("Please respond to the latest user message above.");
    prompt
}
