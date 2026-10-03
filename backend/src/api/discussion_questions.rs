use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};

use crate::db::discussion_questions::{
    self, AnswerDiscussionQuestionRequest, AnswerError, CommentDiscussionQuestionRequest,
    DeclineDiscussionQuestionRequest, DiscussionQuestion, DiscussionQuestionList,
};
use crate::models::{ApiErrorCode, ApiResponse};
use crate::AppState;

/// Launch a workflow declared by a resolved workflow-step question. The
/// question row claims its run id first; creation and spawning happen only for
/// the winning claim, so an HTTP replay observes the existing result.
async fn launch_question_resume(
    state: &AppState,
    discussion_id: &str,
    question_id: &str,
) -> Result<(), String> {
    let claim_discussion_id = discussion_id.to_string();
    let claim_question_id = question_id.to_string();
    let claim = state
        .db
        .with_conn(move |conn| {
            discussion_questions::claim_resume(conn, &claim_discussion_id, &claim_question_id)
                .map_err(anyhow::Error::from)
        })
        .await
        .map_err(|error| error.to_string())?;
    let Some(claim) = claim else {
        return Ok(());
    };

    let launch = crate::core::launch_context::LaunchContext {
        discussion_id: Some(claim.discussion_id.clone()),
        project_id: claim.project_id.clone(),
        triggered_by_run_id: Some(claim.source_run_id.clone()),
        ..Default::default()
    };
    let created = crate::api::workflows::create_manual_run_with_id(
        state,
        &claim.workflow_id,
        claim.variables.clone(),
        std::collections::HashMap::new(),
        launch,
        claim.run_id.clone(),
    )
    .await;

    match created {
        Ok((workflow, run)) => {
            let question_id = claim.question_id.clone();
            let run_id = claim.run_id.clone();
            state
                .db
                .with_conn(move |conn| {
                    discussion_questions::finish_resume(conn, &question_id, &run_id, Ok(()))
                })
                .await
                .map_err(|error| error.to_string())?;
            crate::api::workflows::spawn_manual_run(state, workflow, run, None, true);
            Ok(())
        }
        Err(error) => {
            let question_id = claim.question_id;
            let run_id = claim.run_id;
            let persisted_error = error.clone();
            state
                .db
                .with_conn(move |conn| {
                    discussion_questions::finish_resume(
                        conn,
                        &question_id,
                        &run_id,
                        Err(&persisted_error),
                    )
                })
                .await
                .map_err(|db_error| db_error.to_string())?;
            Err(error)
        }
    }
}

async fn resume_and_reload(
    state: &AppState,
    discussion_id: &str,
    question: DiscussionQuestion,
) -> DiscussionQuestion {
    if let Err(error) = launch_question_resume(state, discussion_id, &question.id).await {
        tracing::warn!(
            discussion_id,
            question_id = %question.id,
            %error,
            "workflow question resume failed"
        );
    }
    let reload_discussion_id = discussion_id.to_string();
    let reload_question_id = question.id.clone();
    state
        .db
        .with_read_conn(move |conn| {
            Ok(discussion_questions::list(conn, &reload_discussion_id)?
                .questions
                .into_iter()
                .find(|candidate| candidate.id == reload_question_id))
        })
        .await
        .ok()
        .flatten()
        .unwrap_or(question)
}

pub async fn list(
    State(state): State<AppState>,
    Path(discussion_id): Path<String>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestionList>>) {
    let result = state
        .db
        .with_read_conn(move |conn| {
            // This endpoint is polled: checking existence must not load the transcript.
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM discussions WHERE id=?1)",
                [&discussion_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Ok(None);
            }
            discussion_questions::list(conn, &discussion_id).map(Some)
        })
        .await;
    match result {
        Ok(Some(list)) => (StatusCode::OK, Json(ApiResponse::ok(list))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion not found",
            )),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

pub async fn answer(
    State(state): State<AppState>,
    Path((discussion_id, question_id)): Path<(String, String)>,
    Json(request): Json<AnswerDiscussionQuestionRequest>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestion>>) {
    let (pseudo, email) = {
        let config = state.config.read().await;
        (
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "Human".into()),
            config.server.avatar_email.clone(),
        )
    };
    let resolution_discussion_id = discussion_id.clone();
    let result = state
        .db
        .with_conn(move |conn| {
            Ok(discussion_questions::answer(
                conn,
                &resolution_discussion_id,
                &question_id,
                &request,
                &pseudo,
                email.as_deref(),
            ))
        })
        .await;
    match result {
        Ok(Ok(question)) => {
            // Wake the durable queue promptly; replay never enqueues a second job.
            state.agent_dispatch_notify.notify_one();
            let question = resume_and_reload(&state, &discussion_id, question).await;
            (StatusCode::OK, Json(ApiResponse::ok(question)))
        }
        Ok(Err(AnswerError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion question not found",
            )),
        ),
        Ok(Err(AnswerError::Conflict)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This question has already been answered",
            )),
        ),
        Ok(Err(AnswerError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(AnswerError::Failed(error))) | Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

/// Refuse an arbitration. Same shape as `answer`: the refusal is a decision,
/// so it is published, routed to the asker and dispatched the same way.
pub async fn decline(
    State(state): State<AppState>,
    Path((discussion_id, question_id)): Path<(String, String)>,
    Json(request): Json<DeclineDiscussionQuestionRequest>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestion>>) {
    let (pseudo, email) = {
        let config = state.config.read().await;
        (
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "Human".into()),
            config.server.avatar_email.clone(),
        )
    };
    let resolution_discussion_id = discussion_id.clone();
    let result = state
        .db
        .with_conn(move |conn| {
            Ok(discussion_questions::decline(
                conn,
                &resolution_discussion_id,
                &question_id,
                &request,
                &pseudo,
                email.as_deref(),
            ))
        })
        .await;
    match result {
        Ok(Ok(question)) => {
            state.agent_dispatch_notify.notify_one();
            let question = resume_and_reload(&state, &discussion_id, question).await;
            (StatusCode::OK, Json(ApiResponse::ok(question)))
        }
        Ok(Err(AnswerError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion question not found",
            )),
        ),
        Ok(Err(AnswerError::Conflict)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This question has already been decided",
            )),
        ),
        Ok(Err(AnswerError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(AnswerError::Failed(error))) | Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

/// Comment on an arbitration without deciding it. The asker receives the text
/// like an answer; the question stays pending.
pub async fn comment(
    State(state): State<AppState>,
    Path((discussion_id, question_id)): Path<(String, String)>,
    Json(request): Json<CommentDiscussionQuestionRequest>,
) -> (StatusCode, Json<ApiResponse<DiscussionQuestion>>) {
    let (pseudo, email) = {
        let config = state.config.read().await;
        (
            config
                .server
                .pseudo
                .clone()
                .unwrap_or_else(|| "Human".into()),
            config.server.avatar_email.clone(),
        )
    };
    let result = state
        .db
        .with_conn(move |conn| {
            Ok(discussion_questions::comment(
                conn,
                &discussion_id,
                &question_id,
                &request,
                &pseudo,
                email.as_deref(),
            ))
        })
        .await;
    match result {
        Ok(Ok(question)) => {
            state.agent_dispatch_notify.notify_one();
            (StatusCode::OK, Json(ApiResponse::ok(question)))
        }
        Ok(Err(AnswerError::NotFound)) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Discussion question not found",
            )),
        ),
        Ok(Err(AnswerError::Conflict)) => (
            StatusCode::CONFLICT,
            Json(ApiResponse::err_coded(
                ApiErrorCode::Conflict,
                "This question has already been decided",
            )),
        ),
        Ok(Err(AnswerError::Invalid(message))) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err_coded(ApiErrorCode::Validation, message)),
        ),
        Ok(Err(AnswerError::Failed(error))) | Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(error.to_string())),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    fn workflow(id: &str, name: &str, variables: serde_json::Value) -> crate::models::Workflow {
        serde_json::from_value(json!({
            "id": id,
            "name": name,
            "project_id": null,
            "trigger": {"type": "Manual"},
            "steps": [{"name": "approval", "step_type": {"type": "Gate"}}],
            "actions": [],
            "safety": {
                "sandbox": false,
                "max_files": null,
                "max_lines": null,
                "require_approval": false
            },
            "workspace_config": null,
            "concurrency_limit": null,
            "variables": variables,
            "enabled": true,
            "created_at": Utc::now(),
            "updated_at": Utc::now()
        }))
        .unwrap()
    }

    fn source_run() -> crate::models::WorkflowRun {
        serde_json::from_value(json!({
            "id": "source-run",
            "workflow_id": "source-workflow",
            "status": "Running",
            "trigger_context": null,
            "step_results": [],
            "tokens_used": 0,
            "workspace_path": null,
            "started_at": Utc::now(),
            "finished_at": null
        }))
        .unwrap()
    }

    fn question_message() -> crate::models::DiscussionMessage {
        crate::models::DiscussionMessage {
            id: "step-question".into(),
            role: crate::models::MessageRole::Agent,
            channel: crate::models::MessageChannel::Main,
            content: "```kronn-question\n{\"version\":1,\"key\":\"resume\",\"question\":\"Continue?\",\"options\":[{\"id\":\"yes\",\"label\":\"Yes\"}],\"resume\":{\"workflow_id\":\"resume-workflow\",\"variables\":{\"ticket\":\"KT-883\"}}}\n```".into(),
            agent_type: Some(crate::models::AgentType::Codex),
            timestamp: Utc::now(),
            tokens_used: 0,
            auth_mode: None,
            model_tier: None,
            cost_usd: None,
            author_pseudo: None,
            author_avatar_email: None,
            source_msg_id: Some("native-step-message".into()),
            duration_ms: None,
            target_agent: None,
            reply_to_message_id: None,
            recovered_partial: false,
            session_tokens_at_message: None,
            author_cli_ordinal: None,
            model: None,
            lint_report: None,
        }
    }

    #[tokio::test]
    async fn resolving_a_step_question_launches_its_declared_workflow_once() {
        let db = std::sync::Arc::new(crate::db::Database::open_in_memory().unwrap());
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::core::config::default_config(),
        ));
        let state = crate::AppState::new_defaults(config, db, crate::DEFAULT_MAX_CONCURRENT_AGENTS);
        state
            .db
            .with_conn(|conn| {
                conn.execute(
                    "INSERT INTO discussions (id, title, agent, created_at, updated_at)
                     VALUES ('room', 'Room', 'Codex', 'now', 'now')",
                    [],
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &workflow("source-workflow", "Implementation", json!([])),
                )?;
                crate::db::workflows::insert_workflow(
                    conn,
                    &workflow(
                        "resume-workflow",
                        "Resume implementation",
                        json!([{"name": "ticket", "label": "Ticket", "placeholder": ""}]),
                    ),
                )?;
                crate::db::workflows::insert_run(conn, &source_run())?;
                crate::db::workflow_step_rooms::begin_activity(
                    conn,
                    "source-run",
                    "orchestrate",
                    "Orchestrate",
                    "room",
                    "Codex",
                )?;
                let session = crate::db::workflow_step_rooms::join(
                    conn,
                    "source-run",
                    "orchestrate",
                    "room",
                    "Codex",
                    "step-session",
                )?;
                crate::db::discussions::insert_cli_message_with_targets_and_dispatches(
                    conn,
                    "room",
                    &question_message(),
                    &[],
                    &[],
                    session.session_pk,
                )?;
                crate::db::workflow_step_rooms::finish_activity(conn, "source-run", "orchestrate")?;
                crate::db::workflow_step_rooms::revoke(conn, &[session.session_pk])?;
                conn.execute(
                    "UPDATE workflow_runs SET status='Success', finished_at='now'
                      WHERE id='source-run'",
                    [],
                )?;
                discussion_questions::answer(
                    conn,
                    "room",
                    "question:step-question:0",
                    &discussion_questions::AnswerDiscussionQuestionRequest {
                        selected_option_ids: vec!["yes".into()],
                        item_answers: vec![],
                        text: None,
                        idempotency_key: "answer-once".into(),
                    },
                    "Romu",
                    None,
                )
                .map_err(anyhow::Error::from)?;
                Ok(())
            })
            .await
            .unwrap();

        launch_question_resume(&state, "room", "question:step-question:0")
            .await
            .unwrap();
        launch_question_resume(&state, "room", "question:step-question:0")
            .await
            .unwrap();

        state
            .db
            .with_read_conn(|conn| {
                let runs: Vec<(String, Option<String>)> = {
                    let mut statement = conn.prepare(
                        "SELECT id, triggered_by_run_id FROM workflow_runs
                          WHERE workflow_id='resume-workflow'",
                    )?;
                    let rows = statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                assert_eq!(runs.len(), 1, "an idempotent replay starts no second run");
                assert_eq!(runs[0].1.as_deref(), Some("source-run"));
                let question = discussion_questions::list(conn, "room")?
                    .questions
                    .into_iter()
                    .next()
                    .unwrap();
                let resume = question.resume.unwrap();
                assert_eq!(
                    resume.state,
                    discussion_questions::DiscussionQuestionResumeState::Launched
                );
                assert_eq!(resume.run_id.as_deref(), Some(runs[0].0.as_str()));
                Ok(())
            })
            .await
            .unwrap();
    }
}
