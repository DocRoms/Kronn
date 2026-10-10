use axum::extract::{Path, State};
use axum::Json;
use std::collections::HashMap;

use crate::core::launch_context::LaunchContext;
use crate::db::discussion_actions::{DiscussionActionKind, DiscussionActionState};
use crate::db::kronn_action_engine::ActionCompletion;
use crate::db::live_page_action_trusts::{
    LivePageActionTrust, LivePageActionTrustRefusal, LivePageActionTrustState,
};
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

/// The latest launch of each row, read on the companion connection: the Page
/// draws its buttons' state from it, and it is polled while any row runs.
pub async fn latest_launches_for_live_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageAction>>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_page_actions::latest_launches_for_live_page(
                crate::db::kronn_action_engine::Reconcile::Projected,
                conn,
                &page_id,
            )
        })
        .await;
    match result {
        Ok(launches) => Json(ApiResponse::ok(launches)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list Page action launches: {error}"),
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct PrefillLivePageActionRequest {
    /// The click's row selectors, as for a launch.
    #[serde(default)]
    pub bindings: HashMap<String, String>,
}

/// The starting values a Page draws from its data for the clicked row's
/// editable fields. Reads only; the launch still takes what the reader sends.
pub async fn prefill(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
    Json(request): Json<PrefillLivePageActionRequest>,
) -> Json<ApiResponse<HashMap<String, String>>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_page_actions::prefill_values(conn, &action_id, &request.bindings)
        })
        .await;
    match result {
        Ok(values) => Json(ApiResponse::ok(values)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to prefill Page action: {error}"),
        )),
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(action_id): Path<String>,
) -> Json<ApiResponse<LivePageAction>> {
    let result = state
        .db
        .with_read_conn(move |conn| {
            crate::db::live_page_actions::get(
                crate::db::kronn_action_engine::Reconcile::Projected,
                conn,
                &action_id,
            )
        })
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
    let step_agents = request.step_agents.unwrap_or_default();
    let trusted = request.trusted == Some(true);
    // A launch without a card runs only what was approved: nothing typed.
    if trusted && (!step_agents.is_empty() || !request.variables.is_empty()) {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "A trusted launch takes no typed value and no agent choice",
        ));
    }
    if !step_agents.is_empty() {
        if let Err(error) = precheck_step_agents(&state, &action_id, &step_agents).await {
            return Json(ApiResponse::err(error));
        }
    }
    let claim_id = action_id.clone();
    let supplied = request.variables;
    let bindings = request.bindings;
    let claimed = state
        .db
        .with_conn(move |conn| {
            if trusted {
                crate::db::live_page_actions::claim_trusted_launch(conn, &claim_id, &bindings)
            } else {
                crate::db::live_page_actions::claim_launch(conn, &claim_id, &supplied, &bindings)
            }
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
        execute_claimed_action(state, action_for_run, variables, step_agents).await;
    });
    Json(ApiResponse::ok(action))
}

/// KT-1025 — agent choices only fit a workflow action; checked before the
/// claim so a refusal leaves the card on its offer.
async fn precheck_step_agents(
    state: &AppState,
    action_id: &str,
    step_agents: &crate::models::StepAgents,
) -> Result<(), String> {
    let lookup_id = action_id.to_string();
    let action = state
        .db
        .with_conn(move |conn| {
            crate::db::live_page_actions::get(
                crate::db::kronn_action_engine::Reconcile::Persisted,
                conn,
                &lookup_id,
            )
        })
        .await
        .map_err(|error| format!("DB error: {error}"))?
        .ok_or_else(|| "Action not found".to_string())?;
    if action.kind != DiscussionActionKind::Workflow {
        return Err("`step_agents` applies only to a workflow action".into());
    }
    crate::api::workflows::precheck_step_agents(state, &action.target_id, step_agents).await
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

/// A trusted launch's run: admitted, inserted and pinned in one transaction
/// against the approval it was claimed with, so no later edit or revocation
/// can make it execute anything else (KT-1029).
async fn start_trusted_run(
    state: &AppState,
    action: &LivePageAction,
    variables: HashMap<String, String>,
    launch: LaunchContext,
) -> Result<crate::models::WorkflowRun, String> {
    let launch_id = action.id.clone();
    let admission: crate::api::workflows::RunAdmission = Box::new(move |conn, snapshot| {
        Ok(
            crate::db::live_page_action_trusts::admit_run(conn, &launch_id, snapshot)?
                .map_err(|refusal| refusal.to_string()),
        )
    });
    let (workflow, run) = crate::api::workflows::create_manual_run_admitted(
        state,
        &action.target_id,
        variables,
        Default::default(),
        launch,
        uuid::Uuid::new_v4().to_string(),
        Some(admission),
    )
    .await?;
    crate::api::workflows::spawn_manual_run(state, workflow, run.clone(), None, false);
    Ok(run)
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
    step_agents: crate::models::StepAgents,
) {
    // Same deterministic context contract as discussion-authored proposals
    // (KT-476): a GLOBAL target launched from this Page still resolves the
    // Page's own project environment/worktree, exactly like a human
    // triggering it directly from that project would.
    let mut launch = LaunchContext::from_live_page(action.project_id.clone());
    launch.step_agents = step_agents;
    match action.kind {
        DiscussionActionKind::QuickPrompt => {
            let response = crate::api::mcp_remote::qp_run(
                State(state.clone()),
                None,
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
                None,
                Json(RunQuickApiRequest {
                    variables,
                    workflow_run_id: None,
                    agent: None,
                    launch: Some(launch.clone()),
                    caller: None,
                }),
            )
            .await
            .0;
            match response.data {
                Some(result) if response.success => {
                    let deep_link = format!("automation:quick_api:{}", result.run_id);
                    let diagnostic = result.error.clone();
                    let preflight_failed = diagnostic
                        .as_deref()
                        .is_some_and(|error| error.starts_with("preflight_failed:"));
                    persist_completion(
                        &state,
                        action.id,
                        ActionCompletion {
                            state: if result.success {
                                DiscussionActionState::Succeeded
                            } else if preflight_failed {
                                DiscussionActionState::PreflightFailed
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
                None,
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
            let started = if action.trusted == Some(true) {
                start_trusted_run(&state, &action, variables, launch.clone()).await
            } else {
                crate::api::workflows::start_manual_run(
                    &state,
                    &action.target_id,
                    variables,
                    Default::default(),
                    None,
                    launch.clone(),
                )
                .await
            };
            match started {
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

/// Every current offer of a Page with its eligibility and human approval.
/// On the writer: an approval found out of date is invalidated as it is read.
pub async fn trusts_for_live_page(
    State(state): State<AppState>,
    Path(page_id): Path<String>,
) -> Json<ApiResponse<Vec<LivePageActionTrustState>>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::live_page_action_trusts::list_for_page(conn, &page_id))
        .await;
    match result {
        Ok(states) => Json(ApiResponse::ok(states)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to list Page action approvals: {error}"),
        )),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct TrustLivePageActionRequest {
    /// The fingerprint the human reviewed; a different current one is refused.
    pub fingerprint: String,
}

/// Human only: a bridge token never reaches this route, and is refused here too.
pub async fn trust(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path(action_id): Path<String>,
    Json(request): Json<TrustLivePageActionRequest>,
) -> Json<ApiResponse<LivePageActionTrust>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can approve a Page action to run without confirmation",
        ));
    }
    let result = state
        .db
        .with_conn(move |conn| {
            crate::db::live_page_action_trusts::approve(conn, &action_id, &request.fingerprint)
        })
        .await;
    match result {
        Ok(trust) => Json(ApiResponse::ok(trust)),
        Err(error) if error.is::<LivePageActionTrustRefusal>() => Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            error.to_string(),
        )),
        Err(error) if error.to_string() == "Action not found" => Json(ApiResponse::err_coded(
            ApiErrorCode::NotFound,
            "Action not found",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to approve Page action: {error}"),
        )),
    }
}

/// Withdraw an approval; the next click opens the card again.
pub async fn revoke_trust(
    State(state): State<AppState>,
    bridge: Option<axum::Extension<crate::core::bridge_token::BridgeCaller>>,
    Path(action_id): Path<String>,
) -> Json<ApiResponse<bool>> {
    if bridge.is_some() {
        return Json(ApiResponse::err_coded(
            ApiErrorCode::Validation,
            "Only a human can change a Page action's approval",
        ));
    }
    let result = state
        .db
        .with_conn(move |conn| crate::db::live_page_action_trusts::revoke(conn, &action_id))
        .await;
    match result {
        Ok(removed) => Json(ApiResponse::ok(removed)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            format!("Unable to withdraw Page action approval: {error}"),
        )),
    }
}

#[cfg(test)]
#[path = "live_page_action_trust_api_tests.rs"]
mod trust_api_tests;
