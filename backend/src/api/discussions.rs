use std::convert::Infallible;
use std::pin::Pin;
use std::time::Duration;
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

/// Maximum title length for discussions (characters).
const MAX_TITLE_LEN: usize = 500;
/// Maximum content/prompt length (bytes, ~100 KB).
const MAX_CONTENT_LEN: usize = 100_000;
/// Global timeout for a single agent stream (30 minutes).
const AGENT_GLOBAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Stall timeout — abort if no output line received for this long (5 minutes).
const AGENT_STALL_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug)]
enum AgentStreamEvent {
    Start,
    Meta { auth_mode: String },
    Chunk { data: serde_json::Value },
    Done { data: serde_json::Value },
    Error { data: serde_json::Value },
    // Orchestration-specific:
    System { data: serde_json::Value },
    Round { data: serde_json::Value },
    AgentStart { data: serde_json::Value },
    AgentDone { data: serde_json::Value },
}

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
    // Input validation
    if req.title.len() > MAX_TITLE_LEN {
        return Json(ApiResponse::err(format!("Title too long ({} chars, max {})", req.title.len(), MAX_TITLE_LEN)));
    }
    if req.initial_prompt.len() > MAX_CONTENT_LEN {
        return Json(ApiResponse::err(format!("Prompt too long ({} bytes, max {})", req.initial_prompt.len(), MAX_CONTENT_LEN)));
    }

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
        profile_ids: req.profile_ids,
        directive_ids: req.directive_ids,
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
    let profile_ids = req.profile_ids;
    let directive_ids = req.directive_ids;
    let project_id = req.project_id;
    match state.db.with_conn(move |conn| {
        // project_id: None = don't change, Some(None) = unset, Some(Some("id")) = set
        let pid_update = project_id.as_ref().map(|p| p.as_deref());
        let mut updated = crate::db::discussions::update_discussion(conn, &id, title.as_deref(), archived, pid_update)?;
        if let Some(ref ids) = skill_ids {
            updated = crate::db::discussions::update_discussion_skill_ids(conn, &id, ids)? || updated;
        }
        if let Some(ref ids) = profile_ids {
            updated = crate::db::discussions::update_discussion_profile_ids(conn, &id, ids)? || updated;
        }
        if let Some(ref ids) = directive_ids {
            updated = crate::db::discussions::update_discussion_directive_ids(conn, &id, ids)? || updated;
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
    // Input validation
    if req.content.len() > MAX_CONTENT_LEN {
        let stream: SseStream = Box::pin(futures::stream::once(async {
            Ok::<_, Infallible>(Event::default().event("error").data(
                serde_json::json!({ "error": "Message too long" }).to_string()
            ))
        }));
        return Sse::new(stream);
    }

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
    let directive_ids = disc.directive_ids.clone();
    let profile_ids = disc.profile_ids.clone();

    let project_path = if let Some(ref pid) = disc.project_id {
        let pid = pid.clone();
        state.db.with_conn(move |conn| {
            let p = crate::db::projects::get_project(conn, &pid)?;
            Ok(p.map(|p| p.path).unwrap_or_default())
        }).await.unwrap_or_default()
    } else {
        String::new()
    };

    // For general discussions (no project), build MCP context from global configs
    let global_mcp_context = if project_path.is_empty() {
        build_global_mcp_context(&state).await
    } else {
        None
    };

    let (tokens, full_access) = {
        let config = state.config.read().await;
        let fa = config.agents.full_access_for(&agent_type);
        (config.tokens.clone(), fa)
    };

    let auth_mode_str = auth_mode_for(&agent_type, &tokens);

    let disc_id = discussion_id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(64);

    // Spawn background task — always saves to DB even if client disconnects
    let semaphore = state.agent_semaphore.clone();
    tokio::spawn(async move {
        // Acquire semaphore permit — limits concurrent agent processes
        let _permit = match semaphore.acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                let _ = tx.send(AgentStreamEvent::Error {
                    data: serde_json::json!({ "error": "Server shutting down" }),
                }).await;
                return;
            }
        };

        let _ = tx.send(AgentStreamEvent::Start).await;
        let _ = tx.send(AgentStreamEvent::Meta { auth_mode: auth_mode_str.clone() }).await;

        match runner::start_agent_with_config(runner::AgentStartConfig {
            agent_type: &agent_type, project_path: &project_path, work_dir: None,
            prompt: &prompt, tokens: &tokens, full_access,
            skill_ids: &skill_ids, directive_ids: &directive_ids, profile_ids: &profile_ids,
            mcp_context_override: global_mcp_context.as_deref(),
        }).await {
            Ok(mut process) => {
                let mut full_response = String::new();
                let mut stream_json_tokens: u64 = 0;
                let is_stream_json = process.output_mode == runner::OutputMode::StreamJson;
                let global_deadline = tokio::time::Instant::now() + AGENT_GLOBAL_TIMEOUT;
                let mut was_interrupted = false;

                while let Some(line) = tokio::select! {
                    line = process.next_line() => line,
                    _ = tokio::time::sleep_until(global_deadline) => {
                        tracing::warn!("Agent stream global timeout ({:?}) exceeded", AGENT_GLOBAL_TIMEOUT);
                        was_interrupted = true;
                        None
                    }
                    _ = async {
                        tokio::time::sleep(AGENT_STALL_TIMEOUT).await
                    } => {
                        tracing::warn!("Agent stream stall timeout ({:?}) — no output", AGENT_STALL_TIMEOUT);
                        was_interrupted = true;
                        None
                    }
                } {
                    // Client disconnected — keep running to save result in DB
                    let client_gone = tx.is_closed();

                    if is_stream_json {
                        match runner::parse_claude_stream_line(&line) {
                            runner::StreamJsonEvent::Text(text) => {
                                full_response.push_str(&text);
                                if !client_gone {
                                    let chunk = serde_json::json!({ "text": text });
                                    let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                                }
                            }
                            runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                stream_json_tokens = stream_json_tokens.max(input_tokens + output_tokens);
                            }
                            runner::StreamJsonEvent::Skip => {}
                        }
                    } else {
                        if !full_response.is_empty() {
                            full_response.push('\n');
                        }
                        full_response.push_str(&line);

                        if !client_gone {
                            let text_with_nl = if full_response.len() > line.len() {
                                format!("\n{}", line)
                            } else {
                                line.clone()
                            };
                            let chunk = serde_json::json!({ "text": text_with_nl });
                            let _ = tx.send(AgentStreamEvent::Chunk { data: chunk }).await;
                        }
                    }
                }

                // Kill agent on timeout/stall (process may still be running)
                if was_interrupted {
                    let _ = process.child.kill().await;
                }

                let status = process.child.wait().await;
                process.fix_ownership();
                let exit_info = match &status {
                    Ok(s) => format!("exit code: {:?}", s.code()),
                    Err(e) => format!("wait error: {}", e),
                };
                let success = !was_interrupted && status.map(|s| s.success()).unwrap_or(false);

                let stderr_lines = process.captured_stderr();
                let stderr_text = stderr_lines.join("\n");

                // Mark partial responses
                if was_interrupted && !full_response.is_empty() {
                    full_response.push_str("\n\n---\n⚠️ [Réponse partielle — agent interrompu]");
                }

                if full_response.is_empty() && !success {
                    tracing::error!(
                        "Agent {:?} exited with error ({}). stderr ({} lines): {}",
                        agent_type, exit_info, stderr_lines.len(),
                        if stderr_text.len() > 500 { &stderr_text[..500] } else { &stderr_text }
                    );
                    if stderr_text.is_empty() {
                        // No output at all — likely auth/session issue
                        full_response = format!(
                            "[Agent exited with error] ({})\n\n\
                            ⚠️ **Aucune sortie capturée.** Causes possibles :\n\
                            - Session expirée → lancez `/login` dans le terminal\n\
                            - Clé API invalide → vérifiez Config > Tokens\n\
                            - Agent non installé ou non trouvé",
                            exit_info
                        );
                    } else {
                        full_response = format!("[Agent exited with error] ({})\n\n{}", exit_info, stderr_text);
                    }
                }

                // Detect error patterns in both stdout and stderr and add helpful guidance
                if !success && !was_interrupted {
                    let all_output = format!("{}\n{}", full_response, stderr_text);
                    let error_hint = detect_agent_error_hint(&all_output);
                    if let Some(hint) = error_hint {
                        full_response.push_str(&format!("\n\n{}", hint));
                    }
                }

                let tokens_used = if stream_json_tokens > 0 {
                    stream_json_tokens
                } else {
                    let (cleaned, count) = runner::parse_token_usage(&agent_type, &full_response, &stderr_lines);
                    if count > 0 {
                        full_response = cleaned;
                    }
                    count
                };

                // Save agent response to DB — always runs even if client is gone
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
                let _ = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &msg)
                }).await;

                let done = serde_json::json!({ "message_id": agent_msg.id, "success": success, "tokens_used": tokens_used });
                let _ = tx.send(AgentStreamEvent::Done { data: done }).await;
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
                let _ = state.db.with_conn(move |conn| {
                    crate::db::discussions::insert_message(conn, &did, &err_msg)
                }).await;

                let err = serde_json::json!({ "error": e });
                let _ = tx.send(AgentStreamEvent::Error { data: err }).await;
            }
        }
    });

    // Thin SSE reader — just maps channel events to SSE
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        while let Some(evt) = rx.recv().await {
            match evt {
                AgentStreamEvent::Start => {
                    yield Event::default().event("start").data("{}");
                }
                AgentStreamEvent::Meta { auth_mode } => {
                    yield Event::default().event("meta").data(
                        serde_json::json!({ "auth_mode": auth_mode }).to_string()
                    );
                }
                AgentStreamEvent::Chunk { data } => {
                    yield Event::default().event("chunk").data(data.to_string());
                }
                AgentStreamEvent::Done { data } => {
                    yield Event::default().event("done").data(data.to_string());
                }
                AgentStreamEvent::Error { data } => {
                    yield Event::default().event("error").data(data.to_string());
                }
                _ => {}
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
    let req_directive_ids = req.directive_ids;
    let req_profile_ids = req.profile_ids;

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
    let orch_directive_ids = if req_directive_ids.is_empty() { disc.directive_ids.clone() } else { req_directive_ids };
    let orch_profile_ids = if req_profile_ids.is_empty() { disc.profile_ids.clone() } else { req_profile_ids };

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

    // For general discussions (no project), build MCP context from global configs
    let global_mcp_context = if project_path.is_empty() {
        build_global_mcp_context(&state).await
    } else {
        None
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

    let disc_id = id.clone();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentStreamEvent>(128);

    // Spawn background task — always saves to DB even if client disconnects
    let semaphore = state.agent_semaphore.clone();
    tokio::spawn(async move {
        // Acquire semaphore permit — limits concurrent agent processes
        let _permit = match semaphore.acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                let _ = tx.send(AgentStreamEvent::Error {
                    data: serde_json::json!({ "error": "Server shutting down" }),
                }).await;
                return;
            }
        };

        // Helper macro to send events — silently drops if client disconnected
        macro_rules! emit {
            ($evt:expr) => {
                if !tx.is_closed() {
                    let _ = tx.send($evt).await;
                }
            };
        }

        let agent_names: Vec<String> = agents.iter().map(agent_display_name).collect();
        let sys_text = format!(
            "Mode orchestration active avec {}. Les agents vont debattre sur {} rounds maximum.",
            agent_names.join(", "), max_rounds
        );
        emit!(AgentStreamEvent::System { data: serde_json::json!({ "text": sys_text, "agents": agent_names }) });

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
            let _ = state.db.with_conn(move |conn| {
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

            emit!(AgentStreamEvent::System { data: serde_json::json!({ "text": match disc_language.as_str() {
                "fr" => "Resume de la conversation en cours...",
                "es" => "Resumiendo la conversacion...",
                _ => "Summarizing conversation...",
            }})});

            let fa = *agent_access.get(&format!("{:?}", primary_agent_type)).unwrap_or(&false);
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &primary_agent_type, project_path: &project_path, work_dir: None,
                prompt: &summary_prompt, tokens: &tokens, full_access: fa,
                skill_ids: &[], directive_ids: &[], profile_ids: &[],
                mcp_context_override: global_mcp_context.as_deref(),
            }).await {
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
            emit!(AgentStreamEvent::Round { data: serde_json::json!({ "round": round, "total": max_rounds }) });

            let mut this_round: Vec<(String, String)> = Vec::new();

            for agent_type in &agents {
                let agent_name = agent_display_name(agent_type);

                emit!(AgentStreamEvent::AgentStart { data: serde_json::json!({ "agent": agent_name, "agent_type": agent_type, "round": round }) });

                let prompt = build_orchestration_prompt(&OrchestrationContext {
                    question: &original_question, current_agent: agent_type, all_agents: &agent_names,
                    previous_rounds: &round_responses, round, max_rounds, lang: &disc_language,
                    conversation_context: &conv_context,
                });

                let fa = *agent_access.get(&format!("{:?}", agent_type)).unwrap_or(&false);
                match runner::start_agent_with_config(runner::AgentStartConfig {
                    agent_type, project_path: &project_path, work_dir: None,
                    prompt: &prompt, tokens: &tokens, full_access: fa,
                    skill_ids: &orch_skill_ids, directive_ids: &orch_directive_ids, profile_ids: &orch_profile_ids,
                    mcp_context_override: global_mcp_context.as_deref(),
                }).await {
                    Ok(mut process) => {
                        let mut full_response = String::new();
                        let mut orch_stream_tokens: u64 = 0;
                        let orch_is_stream_json = process.output_mode == runner::OutputMode::StreamJson;

                        while let Some(line) = process.next_line().await {
                            if orch_is_stream_json {
                                match runner::parse_claude_stream_line(&line) {
                                    runner::StreamJsonEvent::Text(text) => {
                                        full_response.push_str(&text);
                                        if !tx.is_closed() {
                                            let chunk = serde_json::json!({
                                                "text": text, "agent": agent_name,
                                                "agent_type": agent_type, "round": round,
                                            });
                                            emit!(AgentStreamEvent::Chunk { data: chunk });
                                        }
                                    }
                                    runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                        orch_stream_tokens = orch_stream_tokens.max(input_tokens + output_tokens);
                                    }
                                    runner::StreamJsonEvent::Skip => {}
                                }
                            } else {
                                let nl = if full_response.is_empty() { "" } else { "\n" };
                                full_response.push_str(&format!("{}{}", nl, line));
                                if !tx.is_closed() {
                                    let chunk = serde_json::json!({
                                        "text": format!("{}{}", nl, line), "agent": agent_name,
                                        "agent_type": agent_type, "round": round,
                                    });
                                    emit!(AgentStreamEvent::Chunk { data: chunk });
                                }
                            }
                        }

                        let status = process.child.wait().await;
                        process.fix_ownership();
                        let exit_info = match &status {
                            Ok(s) => format!("exit code: {:?}", s.code()),
                            Err(e) => format!("wait error: {}", e),
                        };
                        let orch_success = status.map(|s| s.success()).unwrap_or(false);

                        let orch_stderr = process.captured_stderr();
                        let orch_stderr_text = orch_stderr.join("\n");

                        if full_response.is_empty() && !orch_success {
                            tracing::error!(
                                "Orchestration agent {:?} exited with error ({}). stderr: {}",
                                agent_type, exit_info,
                                if orch_stderr_text.len() > 500 { &orch_stderr_text[..500] } else { &orch_stderr_text }
                            );
                            if orch_stderr_text.is_empty() {
                                full_response = format!("[Agent exited with error] ({})", exit_info);
                            } else {
                                full_response = format!("[Agent exited with error] ({})\n\n{}", exit_info, orch_stderr_text);
                            }
                        } else if full_response.is_empty() {
                            full_response = "[No response]".to_string();
                        }

                        if !orch_success {
                            let all_output = format!("{}\n{}", full_response, orch_stderr_text);
                            if let Some(hint) = detect_agent_error_hint(&all_output) {
                                full_response.push_str(&format!("\n\n{}", hint));
                            }
                        }

                        let tokens_used = if orch_stream_tokens > 0 {
                            orch_stream_tokens
                        } else {
                            let (cleaned, count) = runner::parse_token_usage(agent_type, &full_response, &orch_stderr);
                            if count > 0 { full_response = cleaned; }
                            count
                        };

                        // Save to DB — always runs even if client is gone
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
                            let _ = state.db.with_conn(move |conn| {
                                crate::db::discussions::insert_message(conn, &did, &msg)
                            }).await;
                        }

                        emit!(AgentStreamEvent::AgentDone { data: serde_json::json!({
                            "agent": agent_name, "agent_type": agent_type, "round": round,
                        })});

                        this_round.push((agent_name.clone(), full_response));
                    }
                    Err(e) => {
                        tracing::error!("Orchestration: agent {} failed: {}", agent_name, e);
                        let err_text = format!("[Erreur: {}]", e);
                        this_round.push((agent_name.clone(), err_text));

                        emit!(AgentStreamEvent::AgentDone { data: serde_json::json!({
                            "agent": agent_name, "agent_type": agent_type,
                            "round": round, "error": e,
                        })});
                    }
                }
            }

            round_responses.push(this_round);

            if round >= 2 {
                emit!(AgentStreamEvent::System { data: serde_json::json!({ "text": format!("Round {} termine. Analyse de la convergence...", round) }) });
            }
        }

        // Final synthesis
        {
            let primary_name = agent_display_name(&primary_agent_type);

            emit!(AgentStreamEvent::System { data: serde_json::json!({ "text": format!("{} synthetise le debat...", primary_name) }) });

            emit!(AgentStreamEvent::AgentStart { data: serde_json::json!({ "agent": primary_name, "agent_type": primary_agent_type, "round": "synthesis" }) });

            let synth_prompt = build_synthesis_prompt(&original_question, &round_responses, &disc_language);
            let synth_fa = *agent_access.get(&format!("{:?}", primary_agent_type)).unwrap_or(&false);
            match runner::start_agent_with_config(runner::AgentStartConfig {
                agent_type: &primary_agent_type, project_path: &project_path, work_dir: None,
                prompt: &synth_prompt, tokens: &tokens, full_access: synth_fa,
                skill_ids: &orch_skill_ids, directive_ids: &orch_directive_ids, profile_ids: &orch_profile_ids,
                mcp_context_override: global_mcp_context.as_deref(),
            }).await {
                Ok(mut process) => {
                    let mut full_response = String::new();
                    let mut synth_stream_tokens: u64 = 0;
                    let synth_is_stream_json = process.output_mode == runner::OutputMode::StreamJson;

                    while let Some(line) = process.next_line().await {
                        if synth_is_stream_json {
                            match runner::parse_claude_stream_line(&line) {
                                runner::StreamJsonEvent::Text(text) => {
                                    full_response.push_str(&text);
                                    if !tx.is_closed() {
                                        let chunk = serde_json::json!({
                                            "text": text, "agent": primary_name,
                                            "agent_type": primary_agent_type, "round": "synthesis",
                                        });
                                        emit!(AgentStreamEvent::Chunk { data: chunk });
                                    }
                                }
                                runner::StreamJsonEvent::Usage { input_tokens, output_tokens } => {
                                    synth_stream_tokens = synth_stream_tokens.max(input_tokens + output_tokens);
                                }
                                runner::StreamJsonEvent::Skip => {}
                            }
                        } else {
                            let nl = if full_response.is_empty() { "" } else { "\n" };
                            full_response.push_str(&format!("{}{}", nl, line));
                            if !tx.is_closed() {
                                let chunk = serde_json::json!({
                                    "text": format!("{}{}", nl, line), "agent": primary_name,
                                    "agent_type": primary_agent_type, "round": "synthesis",
                                });
                                emit!(AgentStreamEvent::Chunk { data: chunk });
                            }
                        }
                    }
                    let _ = process.child.wait().await;
                    process.fix_ownership();
                    let synth_stderr = process.captured_stderr();

                    let tokens_used = if synth_stream_tokens > 0 {
                        synth_stream_tokens
                    } else {
                        let (cleaned, count) = runner::parse_token_usage(&primary_agent_type, &full_response, &synth_stderr);
                        if count > 0 { full_response = cleaned; }
                        count
                    };

                    // Save synthesis to DB — always runs even if client is gone
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
                        let _ = state.db.with_conn(move |conn| {
                            crate::db::discussions::insert_message(conn, &did, &msg)
                        }).await;
                    }

                    emit!(AgentStreamEvent::AgentDone { data: serde_json::json!({ "agent": primary_name, "round": "synthesis" }) });
                }
                Err(e) => {
                    tracing::error!("Synthesis failed: {}", e);
                    emit!(AgentStreamEvent::Error { data: serde_json::json!({ "error": format!("Synthesis failed: {}", e) }) });
                }
            }
        }

        emit!(AgentStreamEvent::Done { data: serde_json::json!({ "status": "complete" }) });
    });

    // Thin SSE reader — just maps channel events to SSE
    let stream: SseStream = Box::pin(async_stream::try_stream! {
        while let Some(evt) = rx.recv().await {
            match evt {
                AgentStreamEvent::Start => {
                    yield Event::default().event("start").data("{}");
                }
                AgentStreamEvent::Meta { auth_mode } => {
                    yield Event::default().event("meta").data(
                        serde_json::json!({ "auth_mode": auth_mode }).to_string()
                    );
                }
                AgentStreamEvent::Chunk { data } => {
                    yield Event::default().event("chunk").data(data.to_string());
                }
                AgentStreamEvent::Done { data } => {
                    yield Event::default().event("done").data(data.to_string());
                }
                AgentStreamEvent::Error { data } => {
                    yield Event::default().event("error").data(data.to_string());
                }
                AgentStreamEvent::System { data } => {
                    yield Event::default().event("system").data(data.to_string());
                }
                AgentStreamEvent::Round { data } => {
                    yield Event::default().event("round").data(data.to_string());
                }
                AgentStreamEvent::AgentStart { data } => {
                    yield Event::default().event("agent_start").data(data.to_string());
                }
                AgentStreamEvent::AgentDone { data } => {
                    yield Event::default().event("agent_done").data(data.to_string());
                }
            }
        }
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
    if let Some(pos) = slice.rfind(['.', '!', '?']) {
        return text[..=pos].to_string();
    }
    // Fall back to last word boundary
    if let Some(pos) = slice.rfind(' ') {
        return format!("{}…", &text[..pos]);
    }
    format!("{}…", slice)
}

struct OrchestrationContext<'a> {
    question: &'a str,
    current_agent: &'a AgentType,
    all_agents: &'a [String],
    previous_rounds: &'a [Vec<(String, String)>],
    round: u32,
    max_rounds: u32,
    lang: &'a str,
    conversation_context: &'a str,
}

fn build_orchestration_prompt(ctx: &OrchestrationContext) -> String {
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

    // Include discussion title as context if it's meaningful (not auto-generated placeholder)
    let title_ctx = if !disc.title.is_empty()
        && disc.title != "New discussion"
        && disc.title != "Nouvelle discussion"
        && !disc.title.starts_with("Bootstrap: ")
    {
        format!("Discussion topic: \"{}\"\n\n", disc.title)
    } else {
        String::new()
    };

    let user_msgs: Vec<_> = disc.messages.iter()
        .filter(|m| matches!(m.role, MessageRole::User))
        .collect();

    if user_msgs.len() <= 1 {
        let content = user_msgs.last().map(|m| m.content.clone()).unwrap_or_default();
        return format!("{}\n\n{}{}", lang_instr, title_ctx, content);
    }

    let mut prompt = format!("{}\n\n{}Previous conversation:\n\n", lang_instr, title_ctx);
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

    // Remind the agent about active profiles/skills if they were changed mid-conversation.
    // The profiles are also in the system prompt, but this explicit note in the conversation
    // ensures the agent notices the change even when continuing a long conversation.
    if !disc.profile_ids.is_empty() {
        let profile_names: Vec<String> = disc.profile_ids.iter()
            .map(|id| crate::core::profiles::get_profile(id)
                .map(|p| format!("{} {} ({})", p.avatar, p.persona_name, p.role))
                .unwrap_or_else(|| id.clone()))
            .collect();
        prompt.push_str(&format!(
            "[System note: The user has configured the following agent profiles for this conversation: {}. \
            You MUST respond as these profiles — follow their personas as defined in your system instructions.]\n\n",
            profile_names.join(", ")
        ));
    }

    prompt.push_str("Please respond to the latest user message above.");
    prompt
}

/// Detect common agent error patterns and return a user-friendly hint.
pub(crate) fn detect_agent_error_hint(output: &str) -> Option<String> {
    let lower = output.to_lowercase();

    // Authentication / session errors
    if lower.contains("authentication_error")
        || lower.contains("invalid authentication")
        || lower.contains("api error: 401")
        || lower.contains("unauthorized")
        || lower.contains("invalid x-api-key")
        || lower.contains("not authenticated")
    {
        return Some(
            "⚠️ **Session expirée ou clé API invalide.**\n\
             Reconnectez-vous en lançant `/login` dans le CLI de l'agent concerné.\n\
             Vérifiez aussi vos clés API dans Config > Tokens.".to_string()
        );
    }

    // Rate limiting / overloaded
    if lower.contains("rate_limit") || lower.contains("rate limit")
        || lower.contains("429") || lower.contains("too many requests")
    {
        return Some(
            "⚠️ **Limite de requêtes atteinte (rate limit).**\n\
             Attendez quelques minutes avant de réessayer.\n\
             Status Anthropic : https://status.anthropic.com".to_string()
        );
    }

    // Server overloaded
    if lower.contains("overloaded") || lower.contains("529")
        || lower.contains("capacity") || lower.contains("server_busy")
    {
        return Some(
            "⚠️ **Serveurs surchargés.**\n\
             Les serveurs de l'API sont temporairement saturés. Réessayez dans quelques minutes.\n\
             Status Anthropic : https://status.anthropic.com".to_string()
        );
    }

    // Server errors (500, 502, 503)
    if lower.contains("internal server error") || lower.contains("502 bad gateway")
        || lower.contains("503 service unavailable") || lower.contains("api error: 500")
    {
        return Some(
            "⚠️ **Erreur serveur API.**\n\
             Le service est temporairement indisponible. Réessayez dans quelques minutes.\n\
             Status Anthropic : https://status.anthropic.com".to_string()
        );
    }

    // Credit / billing
    if lower.contains("insufficient_quota") || lower.contains("billing")
        || lower.contains("payment required") || lower.contains("402")
    {
        return Some(
            "⚠️ **Quota épuisé ou problème de facturation.**\n\
             Vérifiez votre abonnement et vos crédits API.".to_string()
        );
    }

    // Network errors
    if lower.contains("econnrefused") || lower.contains("enotfound")
        || lower.contains("network error") || lower.contains("dns resolution")
        || lower.contains("timeout") || lower.contains("timed out")
    {
        return Some(
            "⚠️ **Erreur réseau.**\n\
             Impossible de joindre l'API. Vérifiez votre connexion internet.".to_string()
        );
    }

    // Permission denied (sandbox / file access)
    if lower.contains("permission denied") || lower.contains("sandbox permission") {
        return Some(
            "⚠️ **Permission refusée sur le répertoire du projet.**\n\
             Causes possibles :\n\
             - Le projet n'est pas dans le répertoire rw (`KRONN_REPOS_DIR`)\n\
             - Le container a un UID différent du propriétaire des fichiers → `make stop && make start` pour rebuild\n\
             - Sur macOS : vérifiez que Docker Desktop a accès au répertoire dans Settings > Resources > File sharing".to_string()
        );
    }

    None
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_hint_auth_401() {
        let hint = detect_agent_error_hint("Error: api error: 401 Unauthorized");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("Session expirée"));
    }

    #[test]
    fn error_hint_auth_invalid_key() {
        let hint = detect_agent_error_hint("invalid x-api-key provided");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("/login"));
    }

    #[test]
    fn error_hint_auth_authentication_error() {
        let hint = detect_agent_error_hint("authentication_error: invalid credentials");
        assert!(hint.is_some());
    }

    #[test]
    fn error_hint_rate_limit_429() {
        let hint = detect_agent_error_hint("Error 429: too many requests");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("rate limit"));
    }

    #[test]
    fn error_hint_overloaded_529() {
        let hint = detect_agent_error_hint("Server overloaded (529)");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("surchargés"));
    }

    #[test]
    fn error_hint_server_error_502() {
        let hint = detect_agent_error_hint("502 Bad Gateway");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("indisponible"));
    }

    #[test]
    fn error_hint_server_error_500() {
        let hint = detect_agent_error_hint("API error: 500 Internal Server Error");
        assert!(hint.is_some());
    }

    #[test]
    fn error_hint_billing_402() {
        let hint = detect_agent_error_hint("Error 402 payment required");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("Quota"));
    }

    #[test]
    fn error_hint_billing_insufficient_quota() {
        let hint = detect_agent_error_hint("insufficient_quota: no credits remaining");
        assert!(hint.is_some());
    }

    #[test]
    fn error_hint_network_econnrefused() {
        let hint = detect_agent_error_hint("ECONNREFUSED: connection refused");
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("réseau"));
    }

    #[test]
    fn error_hint_network_timeout() {
        let hint = detect_agent_error_hint("request timed out after 30s");
        assert!(hint.is_some());
    }

    #[test]
    fn error_hint_network_dns() {
        let hint = detect_agent_error_hint("ENOTFOUND api.anthropic.com");
        assert!(hint.is_some());
    }

    #[test]
    fn error_hint_none_for_normal_output() {
        let hint = detect_agent_error_hint("Here is your code:\n```rust\nfn main() {}\n```");
        assert!(hint.is_none());
    }

    #[test]
    fn error_hint_none_for_empty() {
        assert!(detect_agent_error_hint("").is_none());
    }

    #[test]
    fn error_hint_case_insensitive() {
        // Checks that "UNAUTHORIZED" is detected (lowercased)
        let hint = detect_agent_error_hint("UNAUTHORIZED ACCESS DENIED");
        assert!(hint.is_some());
    }
}

/// Build MCP context from global MCP configs for general discussions (no project).
/// Lists the server names so the agent knows which MCP tools are available.
async fn build_global_mcp_context(state: &AppState) -> Option<String> {
    let configs = state.db.with_conn(|conn| {
        crate::db::mcps::list_configs(conn)
    }).await.ok()?;

    let global_configs: Vec<_> = configs.into_iter().filter(|c| c.include_general).collect();
    if global_configs.is_empty() {
        return None;
    }

    // Get server names for the global configs
    let servers = state.db.with_conn(|conn| {
        crate::db::mcps::list_servers(conn)
    }).await.unwrap_or_default();
    let server_map: std::collections::HashMap<String, String> = servers.into_iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();

    let mut result = String::from("## MCP Servers available\n\n");
    result.push_str("You have access to the following MCP servers (global). ");
    result.push_str("Use their tools (prefixed `mcp__<server>__<tool>`) instead of Bash workarounds.\n\n");
    result.push_str("Available servers:\n");
    for cfg in &global_configs {
        let name = server_map.get(&cfg.server_id)
            .cloned()
            .unwrap_or_else(|| cfg.label.clone());
        result.push_str(&format!("- **{}** ({})\n", cfg.label, name));
    }
    result.push('\n');

    Some(result)
}
