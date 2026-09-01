use axum::extract::{Path, State};
use axum::Json;
use std::collections::HashMap;

use crate::core::launch_context::LaunchContext;
use crate::db::discussion_actions::{DiscussionActionKind, DiscussionActionState};
use crate::db::kronn_action_engine::ActionCompletion;
use crate::db::live_page_actions::{
    LaunchLivePageActionRequest, LivePageAction, LivePageActionClaimOutcome,
};
use crate::models::{ApiErrorCode, ApiResponse, RunQuickApiRequest, RunQuickExecRequest};
use crate::AppState;

pub async fn list_for_live_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageAction>>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_page_actions::list_for_live_page(conn, &page_id)
        })
        .await;
    match result {
        Ok(actions) => Json(ApiResponse::ok(actions)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list Page actions: {error}"),
        )),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> Json<ApiResponse<LivePageAction>> {
    let result = state
        .db
        .with_read_conn(move |conn| crate::db::live_page_actions::get(conn, &action_id))
        .await;
    match result {
        Ok(Some(action)) => Json(ApiResponse::ok(action)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Action not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to load Page action: {error}"),
        )),
    }
}

pub async fn cancel(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> Json<ApiResponse<LivePageAction>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::live_page_actions::cancel(conn, &action_id))
        .await;
    match result {
        Ok(Some(action)) => Json(ApiResponse::ok(action)),
        Ok(None) => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Action not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to cancel Page action: {error}"),
        )),
    }
}

pub async fn launch(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
    Json(request): Json<LaunchLivePageActionRequest>,
) -> Json<ApiResponse<LivePageAction>> {
    let claim_id = action_id.clone();
    let supplied = request.variables;
    let bindings = request.bindings;
    let claimed = state
        .db
        .with_conn(move |conn| {
            crate::db::live_page_actions::claim_launch(conn, &claim_id, &supplied, &bindings)
        })
        .await;
    let (action, variables) = match claimed {
        Ok(Some(LivePageActionClaimOutcome::Existing(action))) => {
            return Json(ApiResponse::ok(action));
        }
        Ok(Some(LivePageActionClaimOutcome::Claimed { action, variables })) => (action, variables),
        Ok(None) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::NotFound,
                "Action not found",
            ))
        }
        Err(error) => {
            return Json(ApiResponse::err_coded(
                ApiErrorCode::Validation,
                format!("Preflight failed: {error}"),
            ));
        }
    };

    let action_for_run = action.clone();
    tokio::spawn(async move {
        execute_claimed_action(state, action_for_run, variables).await;
    });
    Json(ApiResponse::ok(action))
}

async fn persist_completion(state: &AppState, action_id: String, completion: ActionCompletion) {
    let logged_action_id = action_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| crate::db::live_page_actions::complete(conn, &action_id, completion))
        .await
    {
        tracing::error!(action_id = %logged_action_id, error = %error, "live page action completion failed");
    }
}

/// A QP launched from a Page creates a discussion rather than a shared run.
/// Persist both sides of that origin/result relationship in one transaction:
/// the action keeps its precise Page/revision anchor and the existing Page ↔
/// discussion registry gains the reverse lookup. A backend restart can then
/// recover the same pair without relying on transient UI state.
async fn persist_qp_completion(
    state: &AppState,
    action_id: String,
    live_page_id: String,
    discussion_id: String,
) {
    let logged_action_id = action_id.clone();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            crate::db::live_page_actions::complete_quick_prompt(
                conn,
                &action_id,
                &live_page_id,
                &discussion_id,
            )
        })
        .await
    {
        tracing::error!(action_id = %logged_action_id, error = %error, "live page QP trace completion failed");
    }
}

async fn execute_claimed_action(
    state: AppState,
    action: LivePageAction,
    variables: HashMap<String, String>,
) {
    // Same deterministic context contract as discussion-authored proposals
    // (KT-476): a GLOBAL target launched from this Page still resolves the
    // Page's own project environment/worktree, exactly like a human
    // triggering it directly from that project would.
    let launch = LaunchContext::from_live_page(action.project_id.clone());
    match action.kind {
        DiscussionActionKind::QuickPrompt => {
            let response = crate::api::mcp_remote::qp_run(
                State(state.clone()),
                Json(crate::api::mcp_remote::McpQpRunRequest {
                    qp_id: action.target_id.clone(),
                    vars: variables,
                    agent: None,
                    project_id: action.project_id.clone(),
                    title: Some(action.target_name.clone()),
                    launch: Some(launch.clone()),
                }),
            )
            .await
            .0;
            match response.data {
                Some(result) if response.success => {
                    persist_qp_completion(&state, action.id, action.live_page_id, result.disc_id)
                        .await;
                }
                _ => {
                    let diagnostic = response
                        .error
                        .unwrap_or_else(|| "Quick Prompt launch failed".into());
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: DiscussionActionState::PreflightFailed,
                            shared_run_id: None,
                            result_discussion_id: None,
                            deep_link: None,
                            diagnostic: Some(diagnostic),
                        },
                    )
                    .await;
                }
            }
        }
        DiscussionActionKind::QuickApi => {
            let response = crate::api::quick_apis::run_qa(
                State(state.clone()),
                Path(action.target_id.clone()),
                Json(RunQuickApiRequest {
                    variables,
                    workflow_run_id: None,
                    agent: None,
                    launch: Some(launch.clone()),
                }),
            )
            .await
            .0;
            match response.data {
                Some(result) if response.success => {
                    let deep_link = format!("automation:quick_api:{}", result.run_id);
                    let diagnostic = result.error.clone();
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: if result.success {
                                DiscussionActionState::Succeeded
                            } else {
                                DiscussionActionState::Failed
                            },
                            shared_run_id: Some(result.run_id),
                            result_discussion_id: None,
                            deep_link: Some(deep_link),
                            diagnostic,
                        },
                    )
                    .await;
                }
                _ => {
                    let diagnostic = response
                        .error
                        .unwrap_or_else(|| "Quick API launch failed".into());
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: DiscussionActionState::Failed,
                            shared_run_id: None,
                            result_discussion_id: None,
                            deep_link: None,
                            diagnostic: Some(diagnostic),
                        },
                    )
                    .await;
                }
            }
        }
        DiscussionActionKind::QuickExec => {
            let response = crate::api::quick_execs::run(
                State(state.clone()),
                Path(action.target_id.clone()),
                Json(RunQuickExecRequest {
                    variables,
                    launch: Some(launch.clone()),
                }),
            )
            .await
            .0;
            match response.data {
                Some(result) if response.success => {
                    let deep_link = format!("automation:quick_exec:{}", result.run_id);
                    let diagnostic = result.error.clone();
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: if result.success {
                                DiscussionActionState::Succeeded
                            } else {
                                DiscussionActionState::Failed
                            },
                            shared_run_id: Some(result.run_id),
                            result_discussion_id: None,
                            deep_link: Some(deep_link),
                            diagnostic,
                        },
                    )
                    .await;
                }
                _ => {
                    let diagnostic = response
                        .error
                        .unwrap_or_else(|| "Quick Exec launch failed".into());
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: DiscussionActionState::Failed,
                            shared_run_id: None,
                            result_discussion_id: None,
                            deep_link: None,
                            diagnostic: Some(diagnostic),
                        },
                    )
                    .await;
                }
            }
        }
        DiscussionActionKind::Workflow => {
            match crate::api::workflows::start_manual_run(
                &state,
                &action.target_id,
                variables,
                None,
                launch.clone(),
            )
            .await
            {
                Ok(run) => {
                    let deep_link = format!("automation:workflow:{}", run.id);
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: DiscussionActionState::Running,
                            shared_run_id: Some(run.id),
                            result_discussion_id: None,
                            deep_link: Some(deep_link),
                            diagnostic: None,
                        },
                    )
                    .await;
                }
                Err(diagnostic) => {
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: DiscussionActionState::PreflightFailed,
                            shared_run_id: None,
                            result_discussion_id: None,
                            deep_link: None,
                            diagnostic: Some(diagnostic),
                        },
                    )
                    .await;
                }
            }
        }
        // Storage-only sentinel: a preflight_failed row created for a block
        // Kronn could never parse. `claim_launch` only claims a `Proposed`
        // row and these never leave `preflight_failed`, so this is
        // unreachable in practice; fail closed defensively instead of
        // panicking if that invariant is ever broken.
        DiscussionActionKind::Invalid => {
            persist_completion(
                &state,
                action.id,
                ActionCompletion {
                    state: DiscussionActionState::Failed,
                    shared_run_id: None,
                    result_discussion_id: None,
                    deep_link: None,
                    diagnostic: Some(
                        "Cette proposition n’a jamais été valide et ne peut pas être lancée."
                            .into(),
                    ),
                },
            )
            .await;
        }
    }
}
