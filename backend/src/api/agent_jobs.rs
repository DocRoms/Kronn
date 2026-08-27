//! KT-335 — backend-owned command jobs and scheduled agent wakes.
//!
//! A native model can end its turn after registering work here: the durable row
//! owns the process, and completion creates an ordinary agent dispatch rooted in
//! a deterministic message. Commands are snapshotted Quick Exec definitions —
//! literal argv, bounded cwd and timeout, never a shell string.

use std::path::Path;

use anyhow::{Context, Result};
use axum::extract::{Path as AxumPath, State};
use axum::Json;
use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::core::quick_exec::{self, QuickExecSpec, QuickExecStatus, Summariser};
use crate::models::{
    AgentResumeFailureKind, AgentResumeJobKind, AgentResumeJobStatus, AgentResumeJobView,
    AgentType, ApiErrorCode, ApiResponse, DiscussionMessage, MessageRole, MessageTarget,
    ScheduleAgentWakeRequest, StartAgentBackgroundJobRequest,
};
use crate::{AppState, CancelGuard};

const MAX_WAKE_DELAY_SECONDS: u32 = 7 * 24 * 60 * 60;
const MAX_REASON_CHARS: usize = 1_000;
const MAX_DEDUPE_CHARS: usize = 160;
const DEFAULT_WAKE_BUDGET: u32 = 3;

pub(crate) struct NativeAgentJobCaller<'a> {
    pub discussion_id: &'a str,
    pub agent_type: &'a AgentType,
    pub source_dispatch_job_id: Option<&'a str>,
    pub workspace_root: Option<&'a Path>,
}

fn validate_common(reason: &str, dedupe_key: &str) -> Result<()> {
    anyhow::ensure!(!reason.trim().is_empty(), "reason is required");
    anyhow::ensure!(
        reason.chars().count() <= MAX_REASON_CHARS,
        "reason exceeds {MAX_REASON_CHARS} characters"
    );
    anyhow::ensure!(!dedupe_key.trim().is_empty(), "dedupe_key is required");
    anyhow::ensure!(
        dedupe_key.chars().count() <= MAX_DEDUPE_CHARS,
        "dedupe_key exceeds {MAX_DEDUPE_CHARS} characters"
    );
    Ok(())
}

fn summariser_for(command: &str, args: &[String]) -> Summariser {
    match command {
        "cargo" if args.iter().any(|arg| arg == "clippy") => Summariser::Clippy,
        "cargo" => Summariser::CargoTest,
        "tsc" => Summariser::Tsc,
        "vitest" => Summariser::Vitest,
        _ => Summariser::Generic,
    }
}

async fn validate_task_link(
    state: &AppState,
    caller: &NativeAgentJobCaller<'_>,
    task_execution_id: Option<&str>,
) -> Result<()> {
    let Some(task_execution_id) = task_execution_id else {
        return Ok(());
    };
    let id = task_execution_id.to_string();
    let discussion_id = caller.discussion_id.to_string();
    let agent_type = caller.agent_type.clone();
    let source_dispatch_job_id = caller.source_dispatch_job_id.map(str::to_string);
    let addressed = state
        .db
        .with_conn(move |conn| {
            let Some(execution) = crate::db::orchestration::get_task_execution(conn, &id)? else {
                return Ok(false);
            };
            if execution.parent_discussion_id == discussion_id {
                return Ok(true);
            }
            let provider_matches = execution
                .worker_agent_type
                .as_deref()
                .map(crate::db::orchestration::agent_type_from_db)
                .transpose()?
                .as_ref()
                == Some(&agent_type);
            Ok(execution.worker_cli_session_id.is_none()
                && execution.sub_discussion_id.as_deref() == Some(discussion_id.as_str())
                && provider_matches
                && execution.dispatch_job_id == source_dispatch_job_id)
        })
        .await?;
    anyhow::ensure!(
        addressed,
        "task execution not found or caller is not a party"
    );
    Ok(())
}

async fn chain_position(
    state: &AppState,
    source_dispatch_job_id: Option<&str>,
) -> Result<(u32, u32)> {
    let Some(dispatch_id) = source_dispatch_job_id else {
        return Ok((0, DEFAULT_WAKE_BUDGET));
    };
    let dispatch_id = dispatch_id.to_string();
    let parent = state
        .db
        .with_conn(move |conn| {
            crate::db::agent_jobs::find_by_completion_dispatch(conn, &dispatch_id)
        })
        .await?;
    let Some(parent) = parent else {
        return Ok((0, DEFAULT_WAKE_BUDGET));
    };
    next_chain_position(parent.view.chain_depth, parent.view.wake_budget)
}

fn next_chain_position(parent_depth: u32, wake_budget: u32) -> Result<(u32, u32)> {
    let next = parent_depth.saturating_add(1);
    anyhow::ensure!(
        next <= wake_budget,
        "agent resume chain budget exhausted ({}/{}) — return control to the user",
        parent_depth,
        wake_budget
    );
    Ok((next, wake_budget))
}

fn scoped_dedupe(caller: &NativeAgentJobCaller<'_>, raw: &str) -> Result<String> {
    let agent = serde_json::to_string(caller.agent_type)?;
    Ok(format!(
        "agent-resume:{}:{}:{}",
        caller.discussion_id,
        agent,
        raw.trim()
    ))
}

pub(crate) async fn start_background_job(
    state: &AppState,
    caller: NativeAgentJobCaller<'_>,
    request: StartAgentBackgroundJobRequest,
) -> Result<AgentResumeJobView> {
    validate_common(&request.reason, &request.dedupe_key)?;
    validate_task_link(state, &caller, request.task_execution_id.as_deref()).await?;

    let quick_exec_id = request.quick_exec_id.trim().to_string();
    anyhow::ensure!(!quick_exec_id.is_empty(), "quick_exec_id is required");
    let lookup_id = quick_exec_id.clone();
    let quick = state
        .db
        .with_conn(move |conn| crate::db::quick_execs::get_quick_exec(conn, &lookup_id))
        .await?
        .context("Quick Exec not found")?;
    crate::api::quick_execs::validate_variables(&quick.variables, &request.variables)
        .map_err(anyhow::Error::msg)?;
    let declared_names = quick
        .variables
        .iter()
        .map(|variable| variable.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    anyhow::ensure!(
        request
            .variables
            .keys()
            .all(|name| declared_names.contains(name.as_str())),
        "variables contain an undeclared Quick Exec input"
    );

    let discussion_id = caller.discussion_id.to_string();
    let discussion_project = state
        .db
        .with_conn(move |conn| {
            Ok(
                crate::db::discussions::get_discussion(conn, &discussion_id)?
                    .and_then(|discussion| discussion.project_id),
            )
        })
        .await?;
    anyhow::ensure!(
        quick.project_id.is_none() || quick.project_id == discussion_project,
        "Quick Exec belongs to another project"
    );

    let workspace_root = caller
        .workspace_root
        .context("this discussion has no bounded workspace")?;
    let mut context = crate::workflows::template::TemplateContext::new();
    for variable in &quick.variables {
        if let Some(value) = request.variables.get(&variable.name) {
            context.set(variable.name.clone(), value.clone());
        }
    }
    let argv = quick
        .args
        .iter()
        .map(|argument| context.render_strict(argument))
        .collect::<Result<Vec<_>>>()?;
    let spec = QuickExecSpec {
        binary: quick.command.clone(),
        argv: argv.clone(),
        cwd: workspace_root.to_path_buf(),
        timeout_secs: Some(u64::from(quick.timeout_secs)),
        stdin: None,
        summariser: summariser_for(&quick.command, &argv),
    };
    quick_exec::validate(&spec, &[workspace_root.to_path_buf()])
        .map_err(|error| anyhow::anyhow!("Quick Exec refused: {error}"))?;

    let (chain_depth, wake_budget) = chain_position(state, caller.source_dispatch_job_id).await?;
    let id = Uuid::new_v4().to_string();
    let dedupe_key = scoped_dedupe(&caller, &request.dedupe_key)?;
    let discussion_id = caller.discussion_id.to_string();
    let target_agent = caller.agent_type.clone();
    let source_dispatch_job_id = caller.source_dispatch_job_id.map(str::to_string);
    let task_execution_id = request.task_execution_id.clone();
    let reason = request.reason.trim().to_string();
    let stored = state
        .db
        .with_conn(move |conn| {
            crate::db::agent_jobs::create(
                conn,
                crate::db::agent_jobs::NewAgentResumeJob {
                    id: &id,
                    discussion_id: &discussion_id,
                    target_agent: &target_agent,
                    source_dispatch_job_id: source_dispatch_job_id.as_deref(),
                    task_execution_id: task_execution_id.as_deref(),
                    quick_exec_id: Some(&quick_exec_id),
                    kind: AgentResumeJobKind::Command,
                    dedupe_key: &dedupe_key,
                    reason: &reason,
                    command_spec: Some(&spec),
                    scheduled_at: Utc::now(),
                    chain_depth,
                    wake_budget,
                },
            )
        })
        .await?;
    state.agent_dispatch_notify.notify_one();
    Ok(stored.view)
}

pub(crate) async fn schedule_wake(
    state: &AppState,
    caller: NativeAgentJobCaller<'_>,
    request: ScheduleAgentWakeRequest,
) -> Result<AgentResumeJobView> {
    validate_common(&request.reason, &request.dedupe_key)?;
    anyhow::ensure!(
        (1..=MAX_WAKE_DELAY_SECONDS).contains(&request.delay_seconds),
        "delay_seconds must be between 1 and {MAX_WAKE_DELAY_SECONDS}"
    );
    validate_task_link(state, &caller, request.task_execution_id.as_deref()).await?;
    let (chain_depth, wake_budget) = chain_position(state, caller.source_dispatch_job_id).await?;
    let id = Uuid::new_v4().to_string();
    let dedupe_key = scoped_dedupe(&caller, &request.dedupe_key)?;
    let discussion_id = caller.discussion_id.to_string();
    let target_agent = caller.agent_type.clone();
    let source_dispatch_job_id = caller.source_dispatch_job_id.map(str::to_string);
    let task_execution_id = request.task_execution_id.clone();
    let reason = request.reason.trim().to_string();
    let scheduled_at = Utc::now() + Duration::seconds(i64::from(request.delay_seconds));
    let stored = state
        .db
        .with_conn(move |conn| {
            crate::db::agent_jobs::create(
                conn,
                crate::db::agent_jobs::NewAgentResumeJob {
                    id: &id,
                    discussion_id: &discussion_id,
                    target_agent: &target_agent,
                    source_dispatch_job_id: source_dispatch_job_id.as_deref(),
                    task_execution_id: task_execution_id.as_deref(),
                    quick_exec_id: None,
                    kind: AgentResumeJobKind::Wake,
                    dedupe_key: &dedupe_key,
                    reason: &reason,
                    command_spec: None,
                    scheduled_at,
                    chain_depth,
                    wake_budget,
                },
            )
        })
        .await?;
    state.agent_dispatch_notify.notify_one();
    Ok(stored.view)
}

fn completion_message(
    job: &crate::db::agent_jobs::AgentResumeJobRecord,
    result: Option<&quick_exec::QuickExecResult>,
) -> DiscussionMessage {
    let result_block = result.map_or_else(
        || "Réveil programmé arrivé à échéance.".to_string(),
        |result| {
            format!(
                "Commande terminée avec le statut `{:?}` (exit {:?}, {} ms).\n\nRésumé borné :\n{}",
                result.status, result.exit_code, result.duration_ms, result.summary
            )
        },
    );
    DiscussionMessage {
        recovered_partial: false,
        session_tokens_at_message: None,
        author_cli_ordinal: None,
        model: None,
        lint_report: None,
        id: format!("agent-resume-result:{}", job.view.id),
        role: MessageRole::User,
        channel: crate::models::MessageChannel::Main,
        content: format!(
            "**Reprise durable Kronn**\n\nJob : `{}`\nRaison : {}\n{}\n\nRelis l’état durable lié avant d’agir ; ne rejoue pas le travail déjà terminé.",
            job.view.id, job.view.reason, result_block
        ),
        agent_type: None,
        timestamp: Utc::now(),
        tokens_used: 0,
        auth_mode: None,
        model_tier: None,
        cost_usd: None,
        author_pseudo: Some("⏱ Kronn · reprise durable".into()),
        author_avatar_email: None,
        source_msg_id: None,
        duration_ms: None,
        target_agent: None,
        reply_to_message_id: None,
    }
}

fn settle_and_dispatch(
    conn: &rusqlite::Connection,
    job: &crate::db::agent_jobs::AgentResumeJobRecord,
    result: Option<&quick_exec::QuickExecResult>,
) -> Result<Option<String>> {
    if !job.view.status.is_active() {
        return Ok(job.view.completion_dispatch_id.clone());
    }
    let transaction = conn.unchecked_transaction()?;
    let current = crate::db::agent_jobs::get(&transaction, &job.view.id)?
        .context("agent resume job vanished before settlement")?;
    if current.view.status != AgentResumeJobStatus::Running {
        return Ok(current.view.completion_dispatch_id);
    }
    let message = completion_message(&current, result);
    let target = MessageTarget::agent(current.view.target_agent.clone());
    let dispatch_id = Uuid::new_v4().to_string();
    let dispatch_dedupe = format!("agent-resume-completion:{}", current.view.id);
    let dispatches = [crate::db::discussions::UserDispatchSpec {
        job_id: &dispatch_id,
        agent_override: Some(&current.view.target_agent),
        dedupe_key: Some(&dispatch_dedupe),
    }];
    let (_, jobs) = crate::db::discussions::insert_message_with_targets_and_dispatches_within_tx(
        &transaction,
        &current.view.discussion_id,
        &message,
        &[target],
        &dispatches,
        None,
    )?;
    let [dispatch] = &jobs[..] else {
        anyhow::bail!(
            "agent resume completion expected one dispatch, got {}",
            jobs.len()
        );
    };
    let terminal_status = match result.map(|value| value.status) {
        None | Some(QuickExecStatus::Passed) => AgentResumeJobStatus::Completed,
        Some(_) => AgentResumeJobStatus::Failed,
    };
    let failure_kind = (terminal_status == AgentResumeJobStatus::Failed)
        .then_some(AgentResumeFailureKind::CommandFailed);
    let last_error = (terminal_status == AgentResumeJobStatus::Failed).then(|| {
        result
            .map(|value| value.summary.as_str())
            .unwrap_or("command failed")
    });
    anyhow::ensure!(
        crate::db::agent_jobs::settle(
            &transaction,
            crate::db::agent_jobs::SettleAgentResumeJob {
                id: &current.view.id,
                terminal_status,
                result,
                failure_kind,
                last_error,
                completion_dispatch_id: &dispatch.id,
            },
        )?,
        "agent resume job changed state during settlement"
    );
    if let Some(execution_id) = current.view.task_execution_id.as_deref() {
        if crate::db::orchestration::get_task_execution(&transaction, execution_id)?
            .is_some_and(|execution| !execution.status.is_terminal())
        {
            crate::db::orchestration::attach_execution_dispatch(
                &transaction,
                execution_id,
                &dispatch.id,
            )?;
        }
    }
    transaction.commit()?;
    Ok(Some(dispatch.id.clone()))
}

async fn run_job(state: AppState, id: String) {
    let claim_id = id.clone();
    let job = match state
        .db
        .with_conn(move |conn| crate::db::agent_jobs::claim(conn, &claim_id))
        .await
    {
        Ok(Some(job)) => job,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(job_id = %id, "agent resume job claim failed: {error}");
            return;
        }
    };

    let result = if job.view.kind == AgentResumeJobKind::Command {
        let Some(spec) = job.command_spec.clone() else {
            release_job(&state, &job.view.id, "command job lost its immutable spec").await;
            return;
        };
        let validated = match quick_exec::validate(&spec, std::slice::from_ref(&spec.cwd)) {
            Ok(validated) => validated,
            Err(error) => {
                release_job(
                    &state,
                    &job.view.id,
                    &format!("snapshotted command refused: {error}"),
                )
                .await;
                return;
            }
        };
        let guard =
            CancelGuard::insert(&state.cancel_registry, format!("agent-job:{}", job.view.id));
        let result = quick_exec::run(&validated, None, &guard.token).await;
        drop(guard);
        match result {
            Ok(result) => Some(result),
            Err(error) => {
                release_job(
                    &state,
                    &job.view.id,
                    &format!("command runtime failed: {error}"),
                )
                .await;
                return;
            }
        }
    } else {
        None
    };

    let settle_job = job.clone();
    let settle_result = result.clone();
    match state
        .db
        .with_conn(move |conn| settle_and_dispatch(conn, &settle_job, settle_result.as_ref()))
        .await
    {
        Ok(Some(_)) => {
            state.agent_dispatch_notify.notify_one();
        }
        Ok(None) => {}
        Err(error) => {
            release_job(
                &state,
                &job.view.id,
                &format!("completion dispatch failed: {error}"),
            )
            .await;
        }
    }
}

async fn release_job(state: &AppState, id: &str, error: &str) {
    let id = id.to_string();
    let error = error.to_string();
    if let Err(db_error) = state
        .db
        .with_conn(move |conn| crate::db::agent_jobs::release_after_error(conn, &id, &error))
        .await
    {
        tracing::error!("agent resume job release failed: {db_error}");
    }
    state.agent_dispatch_notify.notify_one();
}

pub fn start_agent_resume_runner(state: AppState) {
    tokio::spawn(async move {
        loop {
            let ids = state
                .db
                .with_conn(|conn| crate::db::agent_jobs::list_runnable_ids(conn, 32))
                .await
                .unwrap_or_else(|error| {
                    tracing::error!("agent resume job scan failed: {error}");
                    Vec::new()
                });
            for id in ids {
                tokio::spawn(run_job(state.clone(), id));
            }
            tokio::select! {
                _ = state.agent_dispatch_notify.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
            }
        }
    });
}

pub async fn list_for_discussion(
    State(state): State<AppState>,
    AxumPath(discussion_id): AxumPath<String>,
) -> Json<ApiResponse<Vec<AgentResumeJobView>>> {
    let result = state
        .db
        .with_conn(move |conn| crate::db::agent_jobs::list_for_discussion(conn, &discussion_id))
        .await;
    match result {
        Ok(jobs) => Json(ApiResponse::ok(jobs)),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

pub async fn cancel(
    State(state): State<AppState>,
    AxumPath((discussion_id, job_id)): AxumPath<(String, String)>,
) -> Json<ApiResponse<AgentResumeJobView>> {
    let id = job_id.clone();
    let discussion = discussion_id.clone();
    let cancelled = state
        .db
        .with_conn(move |conn| crate::db::agent_jobs::cancel(conn, &id, &discussion))
        .await;
    match cancelled {
        Ok(true) => {
            if let Ok(mut registry) = state.cancel_registry.lock() {
                if let Some(token) = registry.remove(&format!("agent-job:{job_id}")) {
                    token.cancel();
                }
            }
            let lookup_id = job_id;
            match state
                .db
                .with_conn(move |conn| crate::db::agent_jobs::get(conn, &lookup_id))
                .await
            {
                Ok(Some(job)) => Json(ApiResponse::ok(job.view)),
                Ok(None) => Json(ApiResponse::err_coded(
                    ApiErrorCode::NotFound,
                    "agent resume job not found",
                )),
                Err(error) => Json(ApiResponse::err_coded(
                    ApiErrorCode::Internal,
                    error.to_string(),
                )),
            }
        }
        Ok(false) => Json(ApiResponse::err_coded(
            ApiErrorCode::Conflict,
            "agent resume job is not active or belongs to another discussion",
        )),
        Err(error) => Json(ApiResponse::err_coded(
            ApiErrorCode::Internal,
            error.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn common_fields_refuse_empty_and_unbounded_values() {
        assert!(validate_common("", "key").is_err());
        assert!(validate_common("reason", "").is_err());
        assert!(validate_common(&"r".repeat(MAX_REASON_CHARS + 1), "key").is_err());
        assert!(validate_common("reason", &"k".repeat(MAX_DEDUPE_CHARS + 1)).is_err());
    }

    #[test]
    fn wake_chain_budget_allows_n_and_refuses_n_plus_one() {
        assert_eq!(next_chain_position(2, 3).unwrap(), (3, 3));
        let error = next_chain_position(3, 3).unwrap_err().to_string();
        assert!(error.contains("budget exhausted"));
    }

    #[test]
    fn completion_dispatch_is_exactly_once_under_replay() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&connection).unwrap();
        let now = Utc::now().to_rfc3339();
        connection
            .execute(
                "INSERT INTO discussions (id, title, agent, language, created_at, updated_at)
                 VALUES ('d1', 'Resume', 'Ollama', 'fr', ?1, ?1)",
                [&now],
            )
            .unwrap();
        crate::db::agent_jobs::create(
            &connection,
            crate::db::agent_jobs::NewAgentResumeJob {
                id: "resume-once",
                discussion_id: "d1",
                target_agent: &AgentType::Ollama,
                source_dispatch_job_id: None,
                task_execution_id: None,
                quick_exec_id: None,
                kind: AgentResumeJobKind::Wake,
                dedupe_key: "resume:d1:once",
                reason: "external state ready",
                command_spec: None,
                scheduled_at: Utc::now(),
                chain_depth: 0,
                wake_budget: 3,
            },
        )
        .unwrap();
        let running = crate::db::agent_jobs::claim(&connection, "resume-once")
            .unwrap()
            .unwrap();
        let first = settle_and_dispatch(&connection, &running, None)
            .unwrap()
            .unwrap();
        let replay = settle_and_dispatch(&connection, &running, None)
            .unwrap()
            .unwrap();
        assert_eq!(first, replay);
        let messages: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = 'agent-resume-result:resume-once'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let dispatches: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_dispatch_jobs
                 WHERE dedupe_key = 'agent-resume-completion:resume-once'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((messages, dispatches), (1, 1));
    }
}
