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
    match state.db.with_conn(crate::db::discussions::list_discussions).await {
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
        tokens_used: 0,
        auth_mode: None,
    };

    let discussion = Discussion {
        id: Uuid::new_v4().to_string(),
        project_id: req.project_id,
        title: req.title,
        agent: req.agent.clone(),
        language,
        participants: vec![req.agent.clone()],
        messages: vec![initial_message.clone()],
        message_count: 1,
        skill_ids: req.skill_ids,
        archived: false,
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

/// PATCH /api/discussions/:id
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateDiscussionRequest>,
) -> Json<ApiResponse<()>> {
    let title = req.title;
    let archived = req.archived;
    let skill_ids = req.skill_ids;
    match state.db.with_conn(move |conn| {
        let mut updated = crate::db::discussions::update_discussion(conn, &id, title.as_deref(), archived)?;
        if let Some(ref ids) = skill_ids {
            updated = crate::db::discussions::update_discussion_skill_ids(conn, &id, ids)? || updated;
        }
        Ok(updated)
    }).await {
        Ok(true) => Json(ApiResponse::ok(())),
        Ok(false) => Json(ApiResponse::err("Discussion not found or no fields to update")),
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
        tokens_used: 0,
        auth_mode: None,
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
    let skill_ids = disc.skill_ids.clone();

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
        let fa = config.agents.full_access_for(&agent_type);
        (config.tokens.clone(), fa)
    };

    let auth_mode_str = auth_mode_for(&agent_type, &tokens);

    let state_clone = state.clone();
    let disc_id = discussion_id.clone();

    let stream: SseStream = Box::pin(async_stream::try_stream! {
        yield Event::default().event("start").data("{}");

        yield Event::default().event("meta").data(
            serde_json::json!({ "auth_mode": auth_mode_str }).to_string()
        );

        match runner::start_agent_with_skills(&agent_type, &project_path, &prompt, &tokens, full_access, &skill_ids).await {
            Ok(mut process) => {
                let mut full_response = String::new();
                let mut stream_json_tokens: u64 = 0;
                let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;

                while let Some(line) = process.next_line().await {
                    if is_stream_json {
                        // Parse Claude Code stream-json events
                        match runner::parse_claude_stream_line(&line) {
                            runner::StreamJsonEvent::Text(text) => {
                                full_response.push_str(&text);
                                let chunk = serde_json::json!({ "text": text });
                                yield Event::default().event("chunk").data(chunk.to_string());
                            }
                            runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                stream_json_tokens = stream_json_tokens.max(input_tokens + output_tokens);
                            }
                            runner::StreamJsonEvent::Skip => {}
                        }
                    } else {
                        // Plain text mode — each line from stdout is a complete line.
                        // Append newline so the frontend can concatenate chunks directly.
                        if !full_response.is_empty() {
                            full_response.push('\n');
                        }
                        full_response.push_str(&line);

                        let text_with_nl = if full_response.len() > line.len() {
                            format!("\n{}", line)
                        } else {
                            line.clone()
                        };
                        let chunk = serde_json::json!({ "text": text_with_nl });
                        yield Event::default().event("chunk").data(chunk.to_string());
                    }
                }

                let status = process.child.wait().await;
                process.fix_ownership();
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

                let stderr_lines = process.captured_stderr();

                if full_response.is_empty() && !success {
                    // Show captured stderr when agent fails silently
                    if stderr_lines.is_empty() {
                        full_response = "[Agent exited with error]".to_string();
                    } else {
                        full_response = format!("[Agent exited with error]\n\n{}", stderr_lines.join("\n"));
                    }
                }

                // Token usage: stream-json parsed inline, others use parse_token_usage
                let tokens_used = if stream_json_tokens > 0 {
                    stream_json_tokens
                } else {
                    let (cleaned, count) = runner::parse_token_usage(&agent_type, &full_response, &stderr_lines);
                    if count > 0 {
                        full_response = cleaned;
                    }
                    count
                };

                // Save agent response to DB
                let agent_msg = DiscussionMessage {
                    id: Uuid::new_v4().to_string(),
                    role: MessageRole::Agent,
                    content: full_response,
                    agent_type: Some(agent_type.clone()),
                    timestamp: Utc::now(),
                    tokens_used,
                    auth_mode: Some(auth_mode_str.clone()),
                };

                let did = disc_id.clone();
                let msg = agent_msg.clone();
                let _ = state_clone.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &msg)
                }).await;

                let done = serde_json::json!({ "message_id": agent_msg.id, "success": success, "tokens_used": tokens_used });
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
                    tokens_used: 0,
                    auth_mode: None,
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
    let req_skill_ids = req.skill_ids;

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
    // Build raw conversation context (all messages except the last user message being debated)
    // This will be summarized by the primary agent before injection into the debate.
    let raw_conv_context = {
        let msgs = &disc.messages;
        let last_user_idx = msgs.iter().rposition(|m| matches!(m.role, MessageRole::User));
        let prior_msgs: Vec<_> = match last_user_idx {
            Some(idx) => msgs[..idx].to_vec(),
            None => vec![],
        };
        if prior_msgs.is_empty() {
            String::new()
        } else {
            let mut ctx = String::new();
            for msg in &prior_msgs {
                match msg.role {
                    MessageRole::User => ctx.push_str(&format!("User: {}\n\n", msg.content)),
                    MessageRole::Agent => {
                        let label = msg.agent_type.as_ref()
                            .map(agent_display_name)
                            .unwrap_or_else(|| "Agent".into());
                        ctx.push_str(&format!("{}: {}\n\n", label, msg.content));
                    }
                    MessageRole::System => {}
                }
            }
            ctx
        }
    };
    let disc_language = disc.language.clone();
    let primary_agent_type = disc.agent.clone();
    // Use skills from the orchestration request if provided, otherwise fall back to discussion skills
    let orch_skill_ids = if req_skill_ids.is_empty() { disc.skill_ids.clone() } else { req_skill_ids };

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
        let access_map: std::collections::HashMap<String, bool> = agents.iter()
            .map(|a| (format!("{:?}", a), config.agents.full_access_for(a)))
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
                tokens_used: 0,
                auth_mode: None,
            };
            let did = disc_id.clone();
            let _ = state_clone.db.with_conn(move |conn| {
                crate::db::discussions::insert_message(conn, &did, &msg)
            }).await;
        }

        // ── Summarize prior conversation via primary agent (if any) ──────────
        let conv_context = if raw_conv_context.is_empty() {
            String::new()
        } else {
            let summary_prompt = match disc_language.as_str() {
                "fr" => format!(
                    "Resume cette conversation en 3-5 phrases courtes, en conservant uniquement les decisions cles, \
                    les contraintes et le contexte necessaire pour repondre a la derniere question.\n\
                    Ne donne PAS ton avis. Fournis UNIQUEMENT le resume factuel.\n\
                    Reponds en francais.\n\n\
                    Conversation :\n{}",
                    raw_conv_context
                ),
                "es" => format!(
                    "Resume esta conversacion en 3-5 frases cortas, conservando solo las decisiones clave, \
                    las restricciones y el contexto necesario para responder a la ultima pregunta.\n\
                    NO des tu opinion. Proporciona UNICAMENTE el resumen factual.\n\
                    Responde en espanol.\n\n\
                    Conversacion:\n{}",
                    raw_conv_context
                ),
                _ => format!(
                    "Summarize this conversation in 3-5 short sentences, keeping only the key decisions, \
                    constraints and context needed to answer the latest question.\n\
                    Do NOT give your opinion. Provide ONLY the factual summary.\n\
                    Respond in English.\n\n\
                    Conversation:\n{}",
                    raw_conv_context
                ),
            };

            yield Event::default().event("system").data(
                serde_json::json!({ "text": match disc_language.as_str() {
                    "fr" => "Resume de la conversation en cours...",
                    "es" => "Resumiendo la conversacion...",
                    _ => "Summarizing conversation...",
                }}).to_string()
            );

            let fa = *agent_access.get(&format!("{:?}", primary_agent_type)).unwrap_or(&false);
            match runner::start_agent_with_skills(&primary_agent_type, &project_path, &summary_prompt, &tokens, fa, &[]).await {
                Ok(mut process) => {
                    let mut summary = String::new();
                    let is_json = process.output_mode == runner::OutputMode::StreamJson;
                    while let Some(line) = process.next_line().await {
                        if is_json {
                            if let runner::StreamJsonEvent::Text(text) = runner::parse_claude_stream_line(&line) {
                                summary.push_str(&text);
                            }
                        } else {
                            summary.push_str(&line);
                            summary.push('\n');
                        }
                    }
                    let _ = process.child.wait().await;
                    let summary = summary.trim().to_string();
                    if summary.is_empty() { String::new() } else { summary }
                }
                Err(e) => {
                    tracing::warn!("Failed to summarize conversation: {}. Using last messages as fallback.", e);
                    // Fallback: keep the last messages that fit within ~800 chars
                    // (most relevant since they're closest to the debated question)
                    let lines: Vec<&str> = raw_conv_context.split("\n\n").filter(|s| !s.is_empty()).collect();
                    let mut fallback = String::new();
                    for line in lines.iter().rev() {
                        if fallback.len() + line.len() + 2 > 800 { break; }
                        fallback = if fallback.is_empty() {
                            line.to_string()
                        } else {
                            format!("{}\n\n{}", line, fallback)
                        };
                    }
                    fallback
                }
            }
        };

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
                    &original_question, agent_type, &agent_names, &round_responses, round, max_rounds, &disc_language, &conv_context,
                );

                let fa = *agent_access.get(&format!("{:?}", agent_type)).unwrap_or(&false);
                match runner::start_agent_with_skills(agent_type, &project_path, &prompt, &tokens, fa, &orch_skill_ids).await {
                    Ok(mut process) => {
                        let mut full_response = String::new();
                        let mut orch_stream_tokens: u64 = 0;
                        let orch_is_stream_json = process.output_mode == runner::OutputMode::StreamJson;

                        while let Some(line) = process.next_line().await {
                            if orch_is_stream_json {
                                match runner::parse_claude_stream_line(&line) {
                                    runner::StreamJsonEvent::Text(text) => {
                                        full_response.push_str(&text);
                                        let chunk = serde_json::json!({
                                            "text": text, "agent": agent_name,
                                            "agent_type": agent_type, "round": round,
                                        });
                                        yield Event::default().event("chunk").data(chunk.to_string());
                                    }
                                    runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                        orch_stream_tokens = orch_stream_tokens.max(input_tokens + output_tokens);
                                    }
                                    runner::StreamJsonEvent::Skip => {}
                                }
                            } else {
                                let nl = if full_response.is_empty() { "" } else { "\n" };
                                full_response.push_str(&format!("{}{}", nl, line));
                                let chunk = serde_json::json!({
                                    "text": format!("{}{}", nl, line), "agent": agent_name,
                                    "agent_type": agent_type, "round": round,
                                });
                                yield Event::default().event("chunk").data(chunk.to_string());
                            }
                        }

                        let status = process.child.wait().await;
                        process.fix_ownership();
                        let orch_success = status.map(|s| s.success()).unwrap_or(false);

                        let orch_stderr = process.captured_stderr();

                        if full_response.is_empty() && !orch_success {
                            if orch_stderr.is_empty() {
                                full_response = "[Agent exited with error]".to_string();
                            } else {
                                full_response = format!("[Agent exited with error]\n\n{}", orch_stderr.join("\n"));
                            }
                        } else if full_response.is_empty() {
                            full_response = "[No response]".to_string();
                        }

                        // Token usage: stream-json parsed inline, others use parse_token_usage
                        let tokens_used = if orch_stream_tokens > 0 {
                            orch_stream_tokens
                        } else {
                            let (cleaned, count) = runner::parse_token_usage(agent_type, &full_response, &orch_stderr);
                            if count > 0 { full_response = cleaned; }
                            count
                        };

                        // Save to DB
                        {
                            let msg = DiscussionMessage {
                                id: Uuid::new_v4().to_string(),
                                role: MessageRole::Agent,
                                content: full_response.clone(),
                                agent_type: Some(agent_type.clone()),
                                timestamp: Utc::now(),
                                tokens_used,
                                auth_mode: Some(auth_mode_for(agent_type, &tokens)),
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
            match runner::start_agent_with_skills(&primary_agent_type, &project_path, &synth_prompt, &tokens, synth_fa, &orch_skill_ids).await {
                Ok(mut process) => {
                    let mut full_response = String::new();
                    let mut synth_stream_tokens: u64 = 0;
                    let synth_is_stream_json = process.output_mode == runner::OutputMode::StreamJson;

                    while let Some(line) = process.next_line().await {
                        if synth_is_stream_json {
                            match runner::parse_claude_stream_line(&line) {
                                runner::StreamJsonEvent::Text(text) => {
                                    full_response.push_str(&text);
                                    let chunk = serde_json::json!({
                                        "text": text, "agent": primary_name,
                                        "agent_type": primary_agent_type, "round": "synthesis",
                                    });
                                    yield Event::default().event("chunk").data(chunk.to_string());
                                }
                                runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                    synth_stream_tokens = synth_stream_tokens.max(input_tokens + output_tokens);
                                }
                                runner::StreamJsonEvent::Skip => {}
                            }
                        } else {
                            let nl = if full_response.is_empty() { "" } else { "\n" };
                            full_response.push_str(&format!("{}{}", nl, line));
                            let chunk = serde_json::json!({
                                "text": format!("{}{}", nl, line), "agent": primary_name,
                                "agent_type": primary_agent_type, "round": "synthesis",
                            });
                            yield Event::default().event("chunk").data(chunk.to_string());
                        }
                    }
                    let _ = process.child.wait().await;
                    process.fix_ownership();
                    let synth_stderr = process.captured_stderr();

                    // Token usage: stream-json parsed inline, others use parse_token_usage
                    let tokens_used = if synth_stream_tokens > 0 {
                        synth_stream_tokens
                    } else {
                        let (cleaned, count) = runner::parse_token_usage(&primary_agent_type, &full_response, &synth_stderr);
                        if count > 0 { full_response = cleaned; }
                        count
                    };

                    // Save synthesis to DB
                    {
                        let msg = DiscussionMessage {
                            id: Uuid::new_v4().to_string(),
                            role: MessageRole::Agent,
                            content: format!("[Synthese]\n\n{}", full_response),
                            agent_type: Some(primary_agent_type.clone()),
                            timestamp: Utc::now(),
                            tokens_used,
                            auth_mode: Some(auth_mode_for(&primary_agent_type, &tokens)),
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

fn auth_mode_for(agent_type: &AgentType, tokens: &TokensConfig) -> String {
    let provider = match agent_type {
        AgentType::ClaudeCode => "anthropic",
        AgentType::Codex => "openai",
        AgentType::GeminiCli => "google",
        _ => "",
    };
    let has_key = tokens.active_key_for(provider).is_some();
    let is_disabled = tokens.disabled_overrides.iter().any(|d| d == provider);
    if has_key && !is_disabled { "override".to_string() } else { "local".to_string() }
}

fn agent_display_name(agent_type: &AgentType) -> String {
    match agent_type {
        AgentType::ClaudeCode => "Claude Code".into(),
        AgentType::Codex => "Codex".into(),
        AgentType::Vibe => "Vibe".into(),
        AgentType::GeminiCli => "Gemini CLI".into(),
        AgentType::Kiro => "Kiro".into(),
        AgentType::Custom => "Custom".into(),
    }
}

/// Truncate text at the last sentence boundary before `max_len`, falling back to word boundary.
fn smart_truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let slice = &text[..max_len];
    // Try to cut at last sentence end
    if let Some(pos) = slice.rfind(|c| c == '.' || c == '!' || c == '?') {
        return format!("{}", &text[..=pos]);
    }
    // Fall back to last word boundary
    if let Some(pos) = slice.rfind(' ') {
        return format!("{}…", &text[..pos]);
    }
    format!("{}…", slice)
}

fn build_orchestration_prompt(
    question: &str,
    current_agent: &AgentType,
    all_agents: &[String],
    previous_rounds: &[Vec<(String, String)>],
    round: u32,
    max_rounds: u32,
    lang: &str,
    conversation_context: &str,
) -> String {
    let agent_name = agent_display_name(current_agent);

    // Conversation context section (prior exchanges before the debated question)
    let conv_section = if conversation_context.is_empty() {
        String::new()
    } else {
        match lang {
            "fr" => format!("Contexte de la conversation precedente (ne pas repeter) :\n\n{}\n\n", conversation_context),
            "es" => format!("Contexto de la conversacion anterior (no repetir) :\n\n{}\n\n", conversation_context),
            _ => format!("Previous conversation context (do not repeat) :\n\n{}\n\n", conversation_context),
        }
    };

    if round == 1 {
        match lang {
            "fr" => format!(
                "Tu es {} dans un debat technique entre agents IA ({}).\n\
                {}\
                Donne ton point de vue unique sur la question ci-dessous.\n\
                Sois concis et precis (max 200 mots). Ne repete PAS la question.\n\
                Concentre-toi sur ton expertise specifique.\n\
                Reponds en francais.\n\n\
                Question : {}",
                agent_name, all_agents.join(", "), conv_section, question
            ),
            "es" => format!(
                "Eres {} en un debate tecnico entre agentes IA ({}).\n\
                {}\
                Da tu perspectiva unica sobre la pregunta.\n\
                Se conciso y preciso (max 200 palabras). NO repitas la pregunta.\n\
                Responde en espanol.\n\n\
                Pregunta: {}",
                agent_name, all_agents.join(", "), conv_section, question
            ),
            _ => format!(
                "You are {} in a technical debate between AI agents ({}).\n\
                {}\
                Give your unique perspective on the question below.\n\
                Be concise and precise (max 200 words). Do NOT repeat the question.\n\
                Focus on your specific expertise and what you uniquely bring.\n\
                Respond in English.\n\n\
                Question: {}",
                agent_name, all_agents.join(", "), conv_section, question
            ),
        }
    } else {
        let mut ctx = match lang {
            "fr" => format!(
                "Tu es {} au round {}/{} d'un debat technique ({}).\n\
                Voici les echanges precedents :\n\n",
                agent_name, round, max_rounds, all_agents.join(", ")
            ),
            "es" => format!(
                "Eres {} en la ronda {}/{} de un debate tecnico ({}).\n\
                Intercambios anteriores:\n\n",
                agent_name, round, max_rounds, all_agents.join(", ")
            ),
            _ => format!(
                "You are {} in round {}/{} of a technical debate ({}).\n\
                Here are the previous exchanges:\n\n",
                agent_name, round, max_rounds, all_agents.join(", ")
            ),
        };

        if !conversation_context.is_empty() {
            ctx.push_str(&conv_section);
        }

        for (r_idx, round_data) in previous_rounds.iter().enumerate() {
            ctx.push_str(&format!("--- Round {} ---\n", r_idx + 1));
            for (name, response) in round_data {
                let truncated = smart_truncate(response, 500);
                ctx.push_str(&format!("{}: {}\n\n", name, truncated));
            }
        }

        match lang {
            "fr" => ctx.push_str(&format!(
                "Question originale : {}\n\n\
                REGLES IMPORTANTES :\n\
                - Ne repete PAS ce que les autres ont dit. Ne resume PAS les rounds precedents.\n\
                - Ne parle QUE si tu as quelque chose de NOUVEAU : un desaccord, une nuance, une correction.\n\
                - Si tu es d'accord avec tout, reponds juste : \"Je suis d'accord avec le consensus.\" et arrete-toi.\n\
                - Si c'est le round {}/{}, donne ta position FINALE en 1-2 phrases.\n\
                - Max 150 mots.\n\
                Reponds en francais.",
                question, round, max_rounds
            )),
            "es" => ctx.push_str(&format!(
                "Pregunta original: {}\n\n\
                REGLAS IMPORTANTES:\n\
                - NO repitas lo que otros dijeron. NO resumas rondas anteriores.\n\
                - Solo habla si tienes algo NUEVO: un desacuerdo, un matiz, una correccion.\n\
                - Si estas de acuerdo con todo, responde: \"Estoy de acuerdo con el consenso.\" y para.\n\
                - Si es la ronda {}/{}, da tu posicion FINAL en 1-2 frases.\n\
                - Max 150 palabras.\n\
                Responde en espanol.",
                question, round, max_rounds
            )),
            _ => ctx.push_str(&format!(
                "Original question: {}\n\n\
                IMPORTANT RULES:\n\
                - Do NOT repeat what others said. Do NOT summarize previous rounds.\n\
                - Only speak if you have something NEW to add: a disagreement, a nuance, a correction.\n\
                - If you agree with everything said, just state: \"I agree with the consensus.\" and stop.\n\
                - If this is round {}/{}, give your FINAL position in 1-2 sentences.\n\
                - Max 150 words.\n\
                Respond in English.",
                question, round, max_rounds
            )),
        }
        ctx
    }
}

fn build_synthesis_prompt(
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
                    .map(agent_display_name)
                    .unwrap_or_else(|| "Agent".into());
                prompt.push_str(&format!("{}: {}\n\n", agent_label, msg.content));
            }
            MessageRole::System => {}
        }
    }
    prompt.push_str("Please respond to the latest user message above.");
    prompt
}
