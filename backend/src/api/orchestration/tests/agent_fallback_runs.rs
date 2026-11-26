//! KT-1518 — a task worker whose provider cannot serve it continues on the
//! agent's fallback chain, in its own room and worktree; a task failure never
//! moves it. The quota-back decision point of KT-1664 offers the same chain.

use super::*;
use crate::api::discussions::agent_fallback::{
    intercept_failed_turn_among, offer_to_escalated_execution_among, worker_after_quota_among,
};
use crate::core::agent_fallback as fallback;
use crate::models::AgentFallback;

const CLAUDE_LIMIT: &str = "You've hit your limit · resets 11:59pm (UTC)";

fn link(agent: AgentType) -> AgentFallback {
    AgentFallback {
        agent,
        model: None,
        mode: None,
    }
}

fn usable() -> Vec<AgentType> {
    vec![
        AgentType::ClaudeCode,
        AgentType::Codex,
        AgentType::GeminiCli,
        AgentType::Ollama,
    ]
}

/// A state whose config chains Claude Code to `first`.
fn state_with_chain(db: &std::sync::Arc<Database>, first: Option<AgentFallback>) -> AppState {
    let mut config = crate::core::config::default_config();
    config.agents.claude_code.fallback_chain = first.into_iter().collect();
    // The chain of 0.15.3, carried onto the rows the way a start-up does.
    fallback::migrate_legacy_chains(&mut config.agents);
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

async fn the_economy_worker() -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = economy_claude_executions(&db, repo.path(), 1)
        .await
        .remove(0);
    let job = dispatch_of(&db, was.dispatch_job_id.clone().unwrap()).await;
    (repo, db, was, job)
}

/// KT-1991 — a generic worker never moves to an HTTP model: a test of the move
/// onto one prelocalises its execution first, on the `README.md` line
/// `init_repo` commits.
pub(super) async fn prelocalise(db: &Database, exec_id: &str) -> TaskExecution {
    let (id, scope) = (
        exec_id.to_string(),
        serde_json::to_string(&readme_scope()).unwrap(),
    );
    db.with_conn(move |conn| {
        conn.execute(
            "UPDATE task_executions SET worker_scope_json = ?2 WHERE id = ?1",
            rusqlite::params![id, scope],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    exec_of(db, exec_id).await
}

/// KT-1991 — the economy worker with a prelocalised scope: the only kind of
/// execution an HTTP link may take, so the tests of the move itself use it.
async fn the_prelocalised_economy_worker() -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let (repo, db, was, job) = the_economy_worker().await;
    let was = prelocalise(&db, &was.id).await;
    (repo, db, was, job)
}

async fn fallback_notice(db: &Database, was: &TaskExecution, dispatch: &str) -> Option<String> {
    messages_of(db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id == format!("agent-fallback:{dispatch}"))
        .map(|message| message.content)
}

/// The stop line Kronn wrote in `room` for the failed dispatch `dispatch`.
async fn stop_line(db: &Database, room: &str, id: String) -> Option<String> {
    messages_of(db, room)
        .await
        .into_iter()
        .find(|message| message.id == id)
        .map(|message| message.content)
}

/// KT-1991 — the 2026-11-21 case: a generic Claude Code worker is rate limited
/// and its chain holds only an HTTP link (LiteLLM). An HTTP model has no shell:
/// it can neither build, test nor commit, and its hand-back would be recorded as
/// a delivery. The execution stays on its worker and both rooms say why.
#[tokio::test]
async fn a_rate_limited_generic_worker_is_not_moved_to_an_http_link() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::LiteLlm)));
    let mut usable = usable();
    usable.push(AgentType::LiteLlm);

    assert!(
        !intercept_failed_turn_among(
            &state,
            &job,
            "HTTP 429 Too Many Requests: rate_limit_exceeded",
            Some(usable)
        )
        .await,
        "nothing went on elsewhere"
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(after.worker_target_kind, was.worker_target_kind);
    assert_eq!(after.status, was.status, "the status is not touched");
    assert_eq!(
        after.dispatch_job_id, was.dispatch_job_id,
        "no new dispatch"
    );
    assert!(fallback_notice(&db, &was, &job.id).await.is_none());
    let id = was.id.clone();
    assert!(
        db.with_conn(move |conn| fallback::get_execution_origin(conn, &id))
            .await
            .unwrap()
            .is_none(),
        "no move, so no origin"
    );
    // The principal's room and the worker's own both read why.
    let parent = stop_line(
        &db,
        &was.parent_discussion_id,
        format!("agent-fallback-stop:{}", job.id),
    )
    .await
    .expect("the principal's room is told");
    assert!(
        parent.contains("Claude Code: limite de débit atteinte → aucun agent de repli")
            && parent.contains("LiteLLM cannot take this turn: it has no shell"),
        "{parent}"
    );
    let child = was.sub_discussion_id.clone().expect("the worker's room");
    assert_ne!(child, was.parent_discussion_id);
    let worker = stop_line(
        &db,
        &child,
        format!("agent-fallback-stop-worker:{}", job.id),
    )
    .await
    .expect("the worker's room is told");
    assert!(worker.contains("it has no shell"), "{worker}");
}

/// A state whose config sets each `(agent, chain)` the way 0.15.3 stored it,
/// carried onto the mode rows as a start-up does.
fn state_with_chains(
    db: &std::sync::Arc<Database>,
    chains: &[(AgentType, Vec<AgentFallback>)],
) -> AppState {
    let mut config = crate::core::config::default_config();
    for (agent, chain) in chains {
        config
            .agents
            .agent_config_mut(agent)
            .unwrap()
            .fallback_chain = chain.clone();
    }
    fallback::migrate_legacy_chains(&mut config.agents);
    AppState::new_defaults(
        std::sync::Arc::new(tokio::sync::RwLock::new(config)),
        db.clone(),
        crate::DEFAULT_MAX_CONCURRENT_AGENTS,
    )
}

/// One economy execution on `worker`, with `scope` when it is prelocalised.
async fn an_economy_worker_on(
    worker: MessageTarget,
    scope: Option<TaskWorkerScope>,
) -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let (task_reference, parent_discussion_id, _project) = seed(&db, repo.path()).await;
    let execution = provision_single_task_execution_with_scope_and_validations(
        &db,
        ProvisionInput {
            task_reference,
            parent_discussion_id,
            worker: worker.with_tier(crate::models::ModelTier::Economy),
            base_rev: Some("main".into()),
            idempotency_key: Some("kt1991".into()),
        },
        scope,
        Vec::new(),
    )
    .await
    .unwrap();
    let job = dispatch_of(&db, execution.dispatch_job_id.clone().unwrap()).await;
    (repo, db, execution, job)
}

/// The `README.md` line `init_repo` commits, as a prelocalised edit.
fn readme_scope() -> TaskWorkerScope {
    TaskWorkerScope::PrelocalizedEdit {
        path: "README.md".into(),
        start_line: 1,
        end_line: 1,
    }
}

/// KT-1991 — the four paths a task worker's chain is walked from: a failed
/// turn (rate limit, outage, authentication), a quota refusal once the
/// campaign's own fallback is looked at, a launch refusal, and the quota-back
/// decision point of KT-1664 (an escalated execution whose provider still
/// refuses at the re-check, its own entry into the chain's `plan`). `true`:
/// moved.
async fn walk_the_chain(
    path: &str,
    state: &AppState,
    job: &crate::db::agent_dispatch::AgentDispatchJob,
    usable: Vec<AgentType>,
) -> bool {
    use crate::api::discussions::agent_fallback::worker_after_launch_refusal_among;
    match path {
        "failed turn" => {
            intercept_failed_turn_among(
                state,
                job,
                "HTTP 429 Too Many Requests: rate_limit_exceeded",
                Some(usable),
            )
            .await
        }
        "after quota" => worker_after_quota_among(state, job, CLAUDE_LIMIT, Some(usable))
            .await
            .unwrap(),
        "after launch refusal" => worker_after_launch_refusal_among(
            state,
            job,
            fallback::Trigger::LaunchRefused,
            "the agent cannot be launched as a task worker",
            Some(usable),
        )
        .await
        .unwrap()
        .moved(),
        "quota-back decision point" => {
            let dispatch = job.id.clone();
            let execution = state
                .db
                .with_conn(move |conn| {
                    crate::db::orchestration::get_execution_for_dispatch(conn, &dispatch)
                })
                .await
                .unwrap()
                .expect("the dispatch runs an execution");
            offer_to_escalated_execution_among(state, &execution.id, Some(usable))
                .await
                .unwrap()
        }
        other => unreachable!("{other}"),
    }
}

const WORKER_PATHS: [&str; 4] = [
    "failed turn",
    "after quota",
    "after launch refusal",
    "quota-back decision point",
];

#[tokio::test]
async fn a_generic_worker_asks_for_a_shell_on_every_worker_fallback_path() {
    for path in WORKER_PATHS {
        let (_repo, db, was, job) = the_economy_worker().await;
        assert!(was.worker_scope.is_none(), "a generic execution");
        let state = state_with_chain(&db, Some(link(AgentType::LiteLlm)));
        let mut usable = usable();
        usable.push(AgentType::LiteLlm);

        assert!(!walk_the_chain(path, &state, &job, usable).await, "{path}");

        let after = exec_of(&db, &was.id).await;
        assert_eq!(
            after.worker_agent_type.as_deref(),
            Some("ClaudeCode"),
            "{path}"
        );
        assert_eq!(after.status, was.status, "{path}");
        assert_eq!(after.dispatch_job_id, was.dispatch_job_id, "{path}");
        assert!(
            fallback_notice(&db, &was, &job.id).await.is_none(),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_prelocalised_worker_still_falls_back_to_an_http_link_on_every_path() {
    for path in WORKER_PATHS {
        let (_repo, db, was, job) = an_economy_worker_on(
            MessageTarget::discussion_agent(AgentType::LiteLlm),
            Some(readme_scope()),
        )
        .await;
        assert_eq!(was.worker_scope, Some(readme_scope()), "{path}");
        let state = state_with_chains(&db, &[(AgentType::LiteLlm, vec![link(AgentType::Ollama)])]);

        assert!(walk_the_chain(path, &state, &job, usable()).await, "{path}");

        let after = exec_of(&db, &was.id).await;
        assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"), "{path}");
        assert_eq!(after.worker_scope, Some(readme_scope()), "{path}");
        assert_eq!(after.sub_discussion_id, was.sub_discussion_id, "{path}");
        let notice = fallback_notice(&db, &was, &job.id)
            .await
            .unwrap_or_else(|| panic!("{path}: the move is said"));
        assert!(
            notice.contains("LiteLLM · eco mode: ")
                && notice.contains(" → continued with Ollama · eco mode [Local]"),
            "{path}: {notice}"
        );
    }
}

/// KT-2000 — the direct proof on every path, the quota-back decision point
/// included: a generic worker whose chain is [HTTP link, CLI link] skips the
/// HTTP link and is moved to the CLI one.
#[tokio::test]
async fn a_generic_worker_skips_the_http_link_of_a_mixed_chain_for_the_cli_link() {
    for path in WORKER_PATHS {
        // Claude Code is the only CLI a task worker can fall back to (KT-1749),
        // so the worker starts on Codex.
        let (_repo, db, was, job) =
            an_economy_worker_on(MessageTarget::agent(AgentType::Codex), None).await;
        assert!(was.worker_scope.is_none(), "{path}: a generic execution");
        let state = state_with_chains(
            &db,
            &[(
                AgentType::Codex,
                vec![link(AgentType::LiteLlm), link(AgentType::ClaudeCode)],
            )],
        );
        let mut usable = usable();
        usable.push(AgentType::LiteLlm);

        assert!(walk_the_chain(path, &state, &job, usable).await, "{path}");

        let after = exec_of(&db, &was.id).await;
        assert_eq!(
            after.worker_agent_type.as_deref(),
            Some("ClaudeCode"),
            "{path}"
        );
        assert_eq!(
            after.worker_target_kind,
            Some(MessageTargetKind::Agent),
            "{path}"
        );
        assert_eq!(
            after.worker_model_tier.as_deref(),
            Some("economy"),
            "{path}"
        );
        assert_eq!(after.worker_scope, None, "{path}");
        let notice = fallback_notice(&db, &was, &job.id)
            .await
            .unwrap_or_else(|| panic!("{path}: the move is said"));
        // A launch refusal appends its reason after the target.
        assert!(
            notice.contains(" → continued with Claude Code · eco mode [CLI]")
                && !notice.contains("LiteLLM"),
            "{path}: {notice}"
        );
    }
}

#[tokio::test]
async fn a_quota_refused_worker_continues_on_the_fallback_in_its_own_room_with_the_same_tier() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));

    assert!(
        worker_after_quota_among(&state, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_eq!(
        after.worker_model_tier.as_deref(),
        Some("economy"),
        "same tier on the next agent"
    );
    assert_eq!(
        after.worker_model, None,
        "the refused agent's model is dropped"
    );
    assert_eq!(after.sub_discussion_id, was.sub_discussion_id, "same room");
    assert_eq!(after.workspace_id, was.workspace_id, "same worktree");
    assert_eq!(after.child_branch, was.child_branch);
    assert_ne!(
        after.dispatch_job_id, was.dispatch_job_id,
        "a fresh dispatch"
    );

    // The parent room says so, in the agreed words.
    let notice = fallback_notice(&db, &was, &job.id)
        .await
        .expect("room line");
    assert!(
        notice.starts_with(&format!(
            "Task worker execution {}: Claude Code · eco mode: quota exhausted until ",
            was.id
        )) && notice.ends_with(" → continued with Ollama · eco mode [Local]"),
        "{notice}"
    );
    // The refused agent is on cooldown, so new turns start on the fallback.
    let cooling = db
        .with_conn(|conn| {
            fallback::active_cooldown(conn, &AgentType::ClaudeCode, chrono::Utc::now())
        })
        .await
        .unwrap()
        .expect("cooldown");
    assert_eq!(cooling.reason, "quota_exhausted");
    // The provider switch is journaled like any fallback (KT-1564).
    let id = was.id.clone();
    assert!(!db
        .with_conn(move |conn| crate::db::orchestration::provider_changes(conn, &id))
        .await
        .unwrap()
        .is_empty());
}

/// A prelocalised worker: a generic one never moves to an HTTP link (KT-1991).
#[tokio::test]
async fn a_rate_limited_or_unauthenticated_worker_continues_on_the_fallback_too() {
    for response in [
        "HTTP 429 Too Many Requests: rate_limit_exceeded",
        "Invalid authentication credentials (api error: 401)",
        // A saturated provider (503/529) is retried on the same agent first;
        // any other outage goes straight to the chain for a worker.
        "[Agent provider error] API Error: 500 Internal Server Error",
    ] {
        let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
        let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
        assert!(
            intercept_failed_turn_among(&state, &job, response, Some(usable())).await,
            "{response}"
        );
        let after = exec_of(&db, &was.id).await;
        assert_eq!(
            after.worker_agent_type.as_deref(),
            Some("Ollama"),
            "{response}"
        );
        assert_eq!(
            after.worker_model_tier.as_deref(),
            Some("economy"),
            "{response}"
        );
        assert!(
            fallback_notice(&db, &was, &job.id).await.is_some(),
            "{response}"
        );
    }
}

/// KT-1992 — a provider that says its rate limit clears in thirteen minutes.
const SHORT_RATE_LIMIT: &str = "HTTP 429 Too Many Requests: rate_limit_exceeded, retry-after: 780";

/// The prelocalised economy worker with its dispatch claimed: a turn that ran
/// and was refused, the only kind of dispatch a wait can queue again.
async fn the_running_prelocalised_worker() -> (
    tempfile::TempDir,
    std::sync::Arc<Database>,
    TaskExecution,
    crate::db::agent_dispatch::AgentDispatchJob,
) {
    let (repo, db, was, job) = the_prelocalised_economy_worker().await;
    let job = claim_when_due(&db, &job.id).await;
    assert_eq!(job.turn_attempts, 1, "the first run of the turn");
    (repo, db, was, job)
}

/// The line the parent room gets when the worker waits for its agent's reset.
async fn wait_notice(db: &Database, was: &TaskExecution, dispatch: &str) -> Option<String> {
    stop_line(
        db,
        &was.parent_discussion_id,
        format!("agent-fallback-wait:{dispatch}"),
    )
    .await
}

/// KT-1992 — a worker five seconds into its turn was moved to another agent for
/// a limit announced as thirteen minutes long: it now waits on its own agent.
#[tokio::test]
async fn a_worker_rate_limited_with_a_short_announced_reset_waits_on_the_same_agent() {
    let (_repo, db, was, job) = the_running_prelocalised_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    let child = was.sub_discussion_id.clone().expect("the worker's room");
    // The refused reply: a re-claimed dispatch must not find it again.
    let (room, dispatch) = (child.clone(), job.id.clone());
    db.with_conn(move |conn| {
        let mut reply = crate::api::orchestration::orchestrator_message(
            "the-refused-reply".into(),
            SHORT_RATE_LIMIT.into(),
        );
        reply.role = crate::models::MessageRole::System;
        crate::db::discussions::insert_message(conn, &room, &reply)?;
        conn.execute(
            "UPDATE messages SET agent_dispatch_job_id = ?2 WHERE id = ?1",
            rusqlite::params!["the-refused-reply", dispatch],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let before = chrono::Utc::now();

    assert!(
        intercept_failed_turn_among(&state, &job, SHORT_RATE_LIMIT, Some(usable())).await,
        "the turn waits: nothing is left to settle"
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(
        after.worker_agent_type.as_deref(),
        Some("ClaudeCode"),
        "same agent"
    );
    assert_eq!(after.dispatch_job_id, was.dispatch_job_id, "same dispatch");
    assert_eq!(after.status, was.status, "the status is not touched");
    let waiting = dispatch_of(&db, job.id.clone()).await;
    assert_eq!(
        waiting.status,
        crate::db::agent_dispatch::DispatchStatus::Pending
    );
    let ahead = (waiting.available_at - before).num_seconds();
    assert!(
        (13 * 60..=15 * 60).contains(&ahead),
        "the announced thirteen minutes and a minute of margin, got {ahead}s"
    );
    let marker = waiting.last_error.clone().unwrap_or_default();
    assert!(
        marker.starts_with("rate_limit_wait: redispatch at ") && marker.ends_with(" UTC"),
        "{marker}"
    );

    // The principal's room: who waits, until when, and within which limit.
    let parent = wait_notice(&db, &was, &job.id)
        .await
        .expect("the principal's room is told");
    assert!(
        parent.starts_with(&format!(
            "Task worker execution {}: Claude Code · eco mode: rate limited until ",
            was.id
        )) && parent.contains(
            " → waiting for the reset on the same agent (13 min, limit 15 min); the turn restarts at "
        ),
        "{parent}"
    );
    assert!(
        fallback_notice(&db, &was, &job.id).await.is_none(),
        "the worker did not move"
    );
    let id = was.id.clone();
    assert!(
        db.with_conn(move |conn| fallback::get_execution_origin(conn, &id))
            .await
            .unwrap()
            .is_none(),
        "no move, so no origin"
    );

    // The worker's room: the refused reply is gone and the wait is said.
    let room = messages_of(&db, &child).await;
    assert!(
        room.iter().all(|message| message.id != "the-refused-reply"),
        "the refused reply is dropped"
    );
    let notice = room
        .iter()
        .find(|message| {
            message
                .content
                .starts_with("**Rate limited — waiting for the announced reset.**")
        })
        .expect("the worker's room is told");
    assert!(
        notice.content.contains("Claude Code refused this turn")
            && notice.content.contains(
                "13 min from now, within the 15-min wait limit (`workers.rate_limit_wait_max_min`)"
            )
            && notice.content.contains("on the same agent")
            && notice.content.contains("the fallback chain applies"),
        "{}",
        notice.content
    );
}

/// KT-1992 — the threshold: a reset announced beyond the Workers wait limit is
/// not waited for, the worker moves along its chain as before.
#[tokio::test]
async fn a_reset_announced_beyond_the_wait_limit_still_moves_the_worker() {
    let (_repo, db, was, job) = the_running_prelocalised_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));

    assert!(
        intercept_failed_turn_among(
            &state,
            &job,
            "HTTP 429 Too Many Requests: rate_limit_exceeded, retry-after: 7800",
            Some(usable())
        )
        .await
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_ne!(
        after.dispatch_job_id, was.dispatch_job_id,
        "a fresh dispatch"
    );
    assert!(fallback_notice(&db, &was, &job.id).await.is_some());
    assert!(wait_notice(&db, &was, &job.id).await.is_none());
}

/// KT-1992 — `rate_limit_wait_max_min = 0` turns the wait off: even a reset a
/// minute away moves the worker at once.
#[tokio::test]
async fn a_wait_limit_of_zero_keeps_switching_at_once() {
    let (_repo, db, was, job) = the_running_prelocalised_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    state.config.write().await.workers.rate_limit_wait_max_min = 0;

    assert!(
        intercept_failed_turn_among(
            &state,
            &job,
            "HTTP 429 Too Many Requests: rate_limit_exceeded, retry-after: 60",
            Some(usable())
        )
        .await
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert!(fallback_notice(&db, &was, &job.id).await.is_some());
    assert!(wait_notice(&db, &was, &job.id).await.is_none());
}

/// KT-1992 — one wait per turn: the redispatched turn that is refused again
/// follows the chain, whatever reset is announced.
#[tokio::test]
async fn a_second_refusal_after_the_wait_follows_the_chain() {
    let (_repo, db, was, job) = the_running_prelocalised_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    assert!(intercept_failed_turn_among(&state, &job, SHORT_RATE_LIMIT, Some(usable())).await);
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode"),
        "the first refusal is waited out"
    );

    // The reset has passed: the same dispatch runs again and is refused again.
    let again = claim_when_due(&db, &job.id).await;
    assert_eq!(again.turn_attempts, 2);
    assert!(intercept_failed_turn_among(&state, &again, SHORT_RATE_LIMIT, Some(usable())).await);

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_ne!(after.dispatch_job_id, was.dispatch_job_id);
    let moved = fallback_notice(&db, &was, &job.id)
        .await
        .expect("the move is said");
    assert!(
        moved.contains(": rate limited until ")
            && moved.ends_with(" → continued with Ollama · eco mode [Local]"),
        "{moved}"
    );
}

/// KT-1992 — a principal that polls reads the wait and its end, not a silent
/// « in progress »; once the turn runs again nothing is left of it.
#[tokio::test]
async fn status_says_the_worker_waits_for_the_announced_reset() {
    let (_repo, db, was, job) = the_running_prelocalised_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    assert!(intercept_failed_turn_among(&state, &job, SHORT_RATE_LIMIT, Some(usable())).await);

    let id = was.id.clone();
    let detail = db
        .with_conn(move |conn| execution_detail(conn, &id))
        .await
        .unwrap();
    assert_eq!(
        detail.progress.phase,
        crate::models::TaskExecutionProgressPhase::Queued
    );
    let marker = dispatch_of(&db, job.id.clone())
        .await
        .last_error
        .expect("the wait is written on the dispatch");
    assert_eq!(detail.progress.reason.as_deref(), Some(marker.as_str()));
    let restart = marker
        .strip_prefix("rate_limit_wait: redispatch at ")
        .unwrap_or_else(|| panic!("{marker}"));
    let compact = compact_execution_status(&detail, None);
    assert_eq!(
        compact.queued.as_deref(),
        Some(
            format!(
                "waiting for the announced rate-limit reset on the same agent: redispatch at {restart}"
            )
            .as_str()
        )
    );
    assert!(
        compact_status_chars(&compact) <= COMPACT_STATUS_MAX_CHARS,
        "the sentence stays inside the compact budget"
    );

    // The redispatch is due: the turn runs again and the wait is no longer said.
    claim_when_due(&db, &job.id).await;
    let id = was.id.clone();
    let running = db
        .with_conn(move |conn| execution_detail(conn, &id))
        .await
        .unwrap();
    assert!(running
        .progress
        .reason
        .as_deref()
        .is_none_or(|reason| !reason.starts_with("rate_limit_wait")));
    assert!(compact_execution_status(&running, None).queued.is_none());
}

/// KT-1758 — the outage chain takes the worker over from `Working`: the
/// principal is not told to « reassign or cancel » an execution Kronn is
/// reassigning. The replacement that later ends without a delivery posts its
/// own notice (KT-2108: one per dispatch).
#[tokio::test]
async fn an_outage_move_neither_parks_the_worker_nor_spends_the_undelivered_notice() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    // KT-1749: a task worker's chain only offers links the launch accepts (no
    // native ACP agent, no Codex), so the take-over is exercised on an HTTP one.
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    let undelivered = format!("orch-worker-undelivered:{}:", was.id);
    // The notices of this execution: one per dispatch that ended undelivered.
    let notices = |db: std::sync::Arc<Database>, parent: String, prefix: String| async move {
        messages_of(&db, &parent)
            .await
            .into_iter()
            .filter(|message| message.id.starts_with(&prefix))
            .map(|message| (message.id, message.content))
            .collect::<Vec<_>>()
    };

    assert!(
        intercept_failed_turn_among(
            &state,
            &job,
            "[Agent provider error] API Error: 500 Internal Server Error",
            Some(usable())
        )
        .await
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_eq!(
        after.status,
        TaskExecutionStatus::Working,
        "taken over, never parked"
    );
    assert!(
        notices(
            db.clone(),
            was.parent_discussion_id.clone(),
            undelivered.clone()
        )
        .await
        .is_empty(),
        "no « reassign or cancel » before an automatic move"
    );
    assert!(fallback_notice(&db, &was, &job.id).await.is_some());

    // The replacement ends without a delivery: now the principal is told, once.
    let replacement = after.dispatch_job_id.clone().unwrap();
    let (claim_id, child) = (
        replacement.clone(),
        after.sub_discussion_id.clone().unwrap(),
    );
    db.with_conn(move |conn| {
        crate::db::agent_dispatch::claim(conn, &claim_id)?.expect("the replacement is due now");
        crate::api::discussions::runtime::persist_dispatch_settlement(
            conn,
            &claim_id,
            &child,
            None,
            crate::db::workflows::BatchChildOutcome::Failed,
            Some("the replacement crashed"),
            None,
        )
    })
    .await
    .unwrap();
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Interrupted
    );
    let posted = notices(db.clone(), was.parent_discussion_id.clone(), undelivered).await;
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert_eq!(
        posted[0].0,
        format!("orch-worker-undelivered:{}:{replacement}", was.id),
        "the notice is the replacement's own"
    );
    assert!(posted[0].1.contains("réaffecte ou annule"), "{posted:?}");
}

/// KT-1758 — the quota return took the execution while the outage chain was
/// settling the failed dispatch: the chain's reassignment finds the execution
/// on another dispatch and does nothing. The turn the return queued is not
/// cancelled, the origin is not recorded, nothing is posted and nothing is
/// parked.
#[tokio::test]
async fn an_outage_hand_over_beaten_by_the_quota_return_moves_nothing_and_parks_nothing() {
    let (_repo, db, was, job) = the_prelocalised_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    let child = was.sub_discussion_id.clone().unwrap();
    let failed = job.id.clone();
    db.with_conn(move |conn| {
        // What the return queues for the same execution.
        crate::db::agent_dispatch::enqueue_for_latest_user(
            conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: "returned-dispatch",
                discussion_id: &child,
                dedupe_key: "returned-dispatch",
                agent_override: Some(&AgentType::ClaudeCode),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )?;
        // …landing at the very moment the failed dispatch is settled.
        conn.execute_batch(&format!(
            "CREATE TRIGGER the_return_wins AFTER UPDATE OF status ON agent_dispatch_jobs \
             WHEN NEW.id = '{failed}' AND NEW.status = 'Failed' \
             BEGIN UPDATE task_executions SET dispatch_job_id = 'returned-dispatch' \
                   WHERE dispatch_job_id = '{failed}'; END;"
        ))?;
        Ok(())
    })
    .await
    .unwrap();

    assert!(
        intercept_failed_turn_among(
            &state,
            &job,
            "[Agent provider error] API Error: 500 Internal Server Error",
            Some(usable())
        )
        .await,
        "handled: the failed dispatch has nothing left to do"
    );

    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.status, TaskExecutionStatus::Working, "not parked");
    assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(after.dispatch_job_id.as_deref(), Some("returned-dispatch"));
    assert_eq!(
        dispatch_of(&db, "returned-dispatch".into()).await.status,
        crate::db::agent_dispatch::DispatchStatus::Pending,
        "the dispatch the return queued is not cancelled"
    );
    assert!(fallback_notice(&db, &was, &job.id).await.is_none());
    let id = was.id.clone();
    assert!(
        db.with_conn(move |conn| fallback::get_execution_origin(conn, &id))
            .await
            .unwrap()
            .is_none(),
        "the chain's origin is not written for a move that did not happen"
    );
    let undelivered = format!("orch-worker-undelivered:{}:", was.id);
    assert!(messages_of(&db, &was.parent_discussion_id)
        .await
        .iter()
        .all(|message| !message.id.starts_with(&undelivered)));
}

#[tokio::test]
async fn a_task_failure_a_refusal_a_test_failure_or_a_work_timeout_never_moves_a_worker() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    for failure in [
        "test result: FAILED. 12 passed; 3 failed; 0 ignored",
        "I can't help with that request.",
        "error[E0308]: mismatched types — the build failed",
        "Agent stalled (no output for 300s)",
        "Agent stalled (no output for 300s) — last line: HTTP 429 Too Many Requests",
    ] {
        assert!(
            !intercept_failed_turn_among(&state, &job, failure, Some(usable())).await,
            "{failure}"
        );
        assert!(
            !worker_after_quota_among(&state, &job, failure, Some(usable()))
                .await
                .unwrap(),
            "{failure}"
        );
    }
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("ClaudeCode"));
    assert_eq!(after.dispatch_job_id, was.dispatch_job_id);
    assert!(fallback_notice(&db, &was, &job.id).await.is_none());
}

#[tokio::test]
async fn a_worker_whose_agent_has_no_fallback_or_whose_fallback_is_unusable_stays_put() {
    let (_repo, db, was, job) = the_economy_worker().await;
    let none = state_with_chain(&db, None);
    assert!(
        !worker_after_quota_among(&none, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    // A fallback that is not installed is skipped, and the room is told why.
    let missing = state_with_chain(&db, Some(link(AgentType::LiteLlm)));
    assert!(
        !worker_after_quota_among(&missing, &job, CLAUDE_LIMIT, Some(usable()))
            .await
            .unwrap()
    );
    let stop = messages_of(&db, &was.parent_discussion_id)
        .await
        .into_iter()
        .find(|message| message.id == format!("agent-fallback-stop:{}", job.id))
        .expect("the stop is said");
    assert!(
        stop.content
            .contains("Claude Code: quota épuisé → aucun agent de repli")
            && stop
                .content
                .contains("LiteLLM is not installed or not enabled"),
        "{}",
        stop.content
    );
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
}

#[tokio::test]
async fn the_quota_back_decision_point_offers_the_fallback_when_the_provider_still_refuses() {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = escalate_claude_without_reset(&db, repo.path(), 1)
        .await
        .remove(0);
    // KT-1991 — an HTTP link only takes a prelocalised execution.
    let was = prelocalise(&db, &was.id).await;
    let state = state_with_chain(&db, Some(link(AgentType::Ollama)));
    let probe = StubQuotaProbe::new(Some(false));
    let rechecker = rechecker_with(&probe).with_usable_agents(usable());
    let t0 = chrono::Utc::now();

    // Just refused: not asked yet, so nothing moves.
    rechecker.recheck_pass(&state, t0).await;
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Escalated
    );

    // Asked: Claude Code still refuses, so the chain is the alternative to
    // waiting, through the same selection as a failing turn.
    let outcome = rechecker
        .recheck_pass(&state, t0 + chrono::Duration::minutes(21))
        .await;
    assert!(
        outcome.executions.is_empty(),
        "nothing was RESUMED on Claude Code"
    );
    assert_eq!(probe.asked(), vec![AgentType::ClaudeCode]);
    let after = exec_of(&db, &was.id).await;
    assert_eq!(after.worker_agent_type.as_deref(), Some("Ollama"));
    assert_eq!(after.worker_model_tier.as_deref(), Some("economy"));
    assert_ne!(after.status, TaskExecutionStatus::Escalated);
    assert_eq!(after.sub_discussion_id, was.sub_discussion_id);
}

#[tokio::test]
async fn the_quota_back_decision_point_waits_when_there_is_no_chain_or_the_provider_is_silent() {
    let repo = init_repo();
    let db = std::sync::Arc::new(Database::open_in_memory().unwrap());
    let was = escalate_claude_without_reset(&db, repo.path(), 1)
        .await
        .remove(0);
    let t0 = chrono::Utc::now();
    // No chain configured: the escalation waits as it always did.
    let no_chain = state_with_chain(&db, None);
    let still = StubQuotaProbe::new(Some(false));
    let rechecker = rechecker_with(&still).with_usable_agents(usable());
    rechecker.recheck_pass(&no_chain, t0).await;
    rechecker
        .recheck_pass(&no_chain, t0 + chrono::Duration::minutes(21))
        .await;
    assert_eq!(
        exec_of(&db, &was.id).await.status,
        TaskExecutionStatus::Escalated
    );
    // A chain, but the provider said nothing: inconclusive is not "refused".
    let chained = state_with_chain(&db, Some(link(AgentType::Ollama)));
    let silent = StubQuotaProbe::new(None);
    let rechecker = rechecker_with(&silent).with_usable_agents(usable());
    rechecker.recheck_pass(&chained, t0).await;
    rechecker
        .recheck_pass(&chained, t0 + chrono::Duration::minutes(21))
        .await;
    assert_eq!(
        exec_of(&db, &was.id).await.worker_agent_type.as_deref(),
        Some("ClaudeCode")
    );
    // Offered directly with nothing usable to take it: no move either.
    assert!(!offer_to_escalated_execution_among(
        &chained,
        &was.id,
        Some(vec![AgentType::ClaudeCode])
    )
    .await
    .unwrap());
}
