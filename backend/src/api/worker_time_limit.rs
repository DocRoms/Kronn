//! KT-1408 — a worker stopped by the execution time limit must not lose its
//! turn's work.
//!
//! Three pieces share this module:
//! - the warning a worker reads before the limit (`time_limit_notice`), carried
//!   by the results of the Kronn tools it already calls;
//! - the WIP checkpoint taken when the limit stops a worker with a dirty
//!   worktree (`checkpoint_worktree`, recorded by `record_time_limit_stop`);
//! - the note the resume handoff reads back (`handoff_note`).

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::models::{AgentType, TaskExecutionStatus, WorkerCommitHooks};

/// How long before the limit the first warning (`Near`) is due.
const NEAR_LEAD: Duration = Duration::from_secs(30 * 60);
/// How long before the limit the final warning (`Imminent`) is due.
const IMMINENT_LEAD: Duration = Duration::from_secs(10 * 60);
/// Largest patch kept in the execution evidence when no commit could be made.
const PATCH_MAX_BYTES: usize = 64 * 1024;
/// Event action carrying one limit stop; the resume handoff reads it back.
pub const TIME_LIMIT_EVENT: &str = "worker_time_limit";
/// First line of the checkpoint commit, so it is recognisable in `git log`.
pub const CHECKPOINT_SUBJECT_PREFIX: &str = "WIP checkpoint (Kronn)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitStage {
    /// The limit is about 30 minutes away (90 of 120).
    Near,
    /// The limit is about 10 minutes away (110 of 120).
    Imminent,
}

impl LimitStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Near => "near",
            Self::Imminent => "imminent",
        }
    }
}

/// The wall-clock budget of a run for this provider. Mirrors the choice the run
/// loop makes when it arms its deadline (hosted budget, except Ollama's own).
pub fn global_limit(agent: &AgentType, hosted_min: u32, local_min: u32) -> Duration {
    let minutes =
        crate::models::clamp_agent_global_timeout_min(u64::from(if *agent == AgentType::Ollama {
            local_min
        } else {
            hosted_min
        }));
    Duration::from_secs(u64::from(minutes) * 60)
}

/// Where `elapsed` stands against `limit`. The leads (30 and 10 minutes) shrink
/// with a short limit so a 20-minute budget is not warned from its first minute.
pub fn limit_stage(limit: Duration, elapsed: Duration) -> Option<LimitStage> {
    if limit.is_zero() {
        return None;
    }
    let near = limit.saturating_sub(NEAR_LEAD.min(limit / 4));
    let imminent = limit.saturating_sub(IMMINENT_LEAD.min(limit / 12));
    if elapsed >= imminent {
        Some(LimitStage::Imminent)
    } else if elapsed >= near {
        Some(LimitStage::Near)
    } else {
        None
    }
}

fn minutes(duration: Duration) -> u64 {
    duration.as_secs() / 60
}

/// The `time_limit` object a worker finds in a Kronn tool result, or `None`
/// while the limit is still far.
pub fn time_limit_notice(limit: Duration, elapsed: Duration) -> Option<Value> {
    let stage = limit_stage(limit, elapsed)?;
    let remaining = limit.saturating_sub(elapsed);
    let notice = match stage {
        LimitStage::Near => format!(
            "The execution time limit is near: {} of {} min used, about {} min left. Commit \
             what works now with `task_exec_commit`, then deliver with `task_exec_deliver` as \
             soon as the DoD is met.",
            minutes(elapsed),
            minutes(limit),
            minutes(remaining)
        ),
        LimitStage::Imminent => format!(
            "FINAL WARNING: {} of {} min used, about {} min left before Kronn stops you. Stop \
             exploring: commit your changes with `task_exec_commit` and call \
             `task_exec_deliver` NOW. Work left uncommitted at the limit is only checkpointed \
             (WIP), never delivered.",
            minutes(elapsed),
            minutes(limit),
            minutes(remaining)
        ),
    };
    Some(json!({
        "stage": stage.as_str(),
        "elapsed_min": minutes(elapsed),
        "limit_min": minutes(limit),
        "remaining_min": minutes(remaining),
        "notice": notice,
    }))
}

/// The notice for the worker running `dispatch_job_id`, measured from the moment
/// its agent actually started (queue time is not worker time).
pub async fn worker_time_limit_notice(
    state: &crate::AppState,
    dispatch_job_id: &str,
    agent: &AgentType,
) -> Option<Value> {
    let id = dispatch_job_id.to_string();
    let dispatch = state
        .db
        .with_read_conn(move |conn| crate::db::agent_dispatch::get(conn, &id))
        .await
        .ok()
        .flatten()?;
    let started = dispatch.agent_started_at.or(dispatch.claimed_at)?;
    let elapsed = (chrono::Utc::now() - started).to_std().unwrap_or_default();
    let limit = {
        let config = state.config.read().await;
        global_limit(
            agent,
            config.server.agent_global_timeout_min,
            config.server.local_agent_global_timeout_min,
        )
    };
    time_limit_notice(limit, elapsed)
}

/// What the limit stop left in the worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Paths that were uncommitted when the worker was stopped.
    pub files: Vec<String>,
    /// The WIP commit on the task branch, when one could be made.
    pub commit: Option<String>,
    /// `git diff HEAD` kept as evidence when no commit could be made.
    pub patch: Option<String>,
    pub patch_truncated: bool,
    /// Why no commit was made.
    pub commit_error: Option<String>,
}

fn checkpoint_message(reference: &str, files: usize, elapsed: Duration, limit: Duration) -> String {
    format!(
        "{CHECKPOINT_SUBJECT_PREFIX}: {reference} worker stopped by the {}-minute execution limit\n\n\
         Uncommitted work of a worker stopped after {} min ({files} path(s)). This is not a \
         delivery: the resumed worker reviews it and builds on it.\n\n\
         Kronn-Checkpoint: execution-time-limit",
        minutes(limit),
        minutes(elapsed),
    )
}

fn diff_patch(root: &Path) -> Option<(String, bool)> {
    let output = crate::core::lane_git::git(root)
        .ok()?
        .args([
            "diff",
            "--ignore-submodules=dirty",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--binary",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let mut patch = output.stdout;
    let truncated = patch.len() > PATCH_MAX_BYTES;
    patch.truncate(PATCH_MAX_BYTES);
    Some((String::from_utf8_lossy(&patch).into_owned(), truncated))
}

/// Checkpoint a worktree whose worker the limit just stopped. `None` when it is
/// clean. The WIP commit is signed off like every Kronn-mediated commit; when it
/// cannot be made (an unfinished merge, a hook refusing it…) the worktree stays
/// as it is and a patch of the tracked changes goes into the evidence instead.
pub fn checkpoint_worktree(
    root: &Path,
    reference: &str,
    elapsed: Duration,
    limit: Duration,
    data_dir_lock: Option<&std::fs::File>,
) -> Result<Option<Checkpoint>, String> {
    checkpoint_worktree_with_hooks(
        root,
        reference,
        elapsed,
        limit,
        data_dir_lock,
        WorkerCommitHooks::default(),
    )
}

/// [`checkpoint_worktree`] under the project's worker commit hook policy
/// (KT-1745).
pub fn checkpoint_worktree_with_hooks(
    root: &Path,
    reference: &str,
    elapsed: Duration,
    limit: Duration,
    data_dir_lock: Option<&std::fs::File>,
    hooks: WorkerCommitHooks,
) -> Result<Option<Checkpoint>, String> {
    let dirty = crate::core::worktree::worktree_dirty_files(root)?;
    if dirty.is_empty() {
        return Ok(None);
    }
    let files: Vec<String> = dirty.into_iter().map(|file| file.path).collect();
    let message = checkpoint_message(reference, files.len(), elapsed, limit);
    let committed = if crate::api::git_ops::merge_in_progress(root) {
        Err("a merge is in progress in the worktree".to_string())
    } else {
        crate::api::git_ops::run_git_commit_for_worker(
            root,
            &files,
            &message,
            false,
            false,
            data_dir_lock,
            hooks,
        )
    };
    Ok(Some(match committed {
        Ok(commit) => Checkpoint {
            files,
            commit: Some(commit.hash),
            patch: None,
            patch_truncated: false,
            commit_error: None,
        },
        Err(error) => {
            let patch = diff_patch(root);
            Checkpoint {
                files,
                commit: None,
                patch_truncated: patch.as_ref().is_some_and(|(_, truncated)| *truncated),
                patch: patch.map(|(text, _)| text),
                commit_error: Some(error),
            }
        }
    }))
}

/// Record that the limit stopped the worker of `dispatch_job_id`: how long that
/// generation ran and what was checkpointed. Returns whether an execution was
/// found for the dispatch.
pub fn record_time_limit_stop(
    conn: &Connection,
    dispatch_job_id: &str,
    elapsed: Duration,
    limit: Duration,
    checkpoint: Option<&Checkpoint>,
) -> Result<bool> {
    let Some(execution) =
        crate::db::orchestration::get_execution_for_dispatch(conn, dispatch_job_id)?
    else {
        return Ok(false);
    };
    record_stop_event(
        conn,
        &execution.id,
        dispatch_job_id,
        elapsed,
        limit,
        checkpoint,
    )?;
    Ok(true)
}

pub fn record_stop_event(
    conn: &Connection,
    execution_id: &str,
    dispatch_job_id: &str,
    elapsed: Duration,
    limit: Duration,
    checkpoint: Option<&Checkpoint>,
) -> Result<()> {
    let previous: i64 = conn.query_row(
        "SELECT COUNT(*) FROM task_execution_events \
         WHERE task_execution_id = ?1 AND action = ?2",
        rusqlite::params![execution_id, TIME_LIMIT_EVENT],
        |row| row.get(0),
    )?;
    let mut changes = json!({
        "generation": previous + 1,
        "dispatch_job_id": dispatch_job_id,
        "elapsed_secs": elapsed.as_secs(),
        "limit_secs": limit.as_secs(),
    });
    if let Some(checkpoint) = checkpoint {
        changes["files"] = json!(checkpoint.files);
        changes["checkpoint_commit"] = json!(checkpoint.commit);
        changes["patch"] = json!(checkpoint.patch);
        changes["patch_truncated"] = json!(checkpoint.patch_truncated);
        changes["commit_error"] = json!(checkpoint.commit_error);
    }
    crate::db::orchestration::record_execution_event(
        conn,
        execution_id,
        TIME_LIMIT_EVENT,
        Some(TaskExecutionStatus::Working),
        Some(TaskExecutionStatus::Working),
        &crate::models::PlanningActor {
            kind: crate::models::PlanningActorKind::Backend,
            id: Some("orchestrator".to_string()),
            session_id: None,
            source_message_id: None,
        },
        changes,
    )
}

/// The run loop's hook: the limit has just stopped the worker of
/// `dispatch_job_id`, whose process is already dead. Checkpoints its worktree and
/// records the stop. Best effort — a failure is logged, never raised: the
/// worker's dispatch must settle either way.
pub async fn checkpoint_stopped_worker(
    state: &crate::AppState,
    dispatch_job_id: &str,
    elapsed: Duration,
    limit: Duration,
) {
    let id = dispatch_job_id.to_string();
    let lookup = state
        .db
        .with_read_conn(move |conn| {
            let Some(execution) = crate::db::orchestration::get_execution_for_dispatch(conn, &id)?
            else {
                return Ok(None);
            };
            let workspace =
                crate::db::discussion_workspaces::get_managed_for_execution(conn, &execution.id)?;
            let hooks = crate::db::projects::worker_commit_hooks(
                conn,
                workspace
                    .as_ref()
                    .map(|workspace| workspace.project_id.as_str()),
            );
            let root = workspace.and_then(|workspace| workspace.canonical_path);
            let task_number: i64 = conn.query_row(
                "SELECT t.task_number FROM task_executions e \
                 JOIN planning_tasks t ON t.id = e.task_id WHERE e.id = ?1",
                [&execution.id],
                |row| row.get(0),
            )?;
            let reference = format!("KT-{task_number}");
            Ok(Some((root, reference, hooks)))
        })
        .await;
    let (root, reference, hooks) = match lookup {
        Ok(Some(found)) => found,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                dispatch_job_id,
                "time-limit checkpoint lookup failed: {error}"
            );
            return;
        }
    };
    let checkpoint = match root {
        Some(root) => {
            let lock = state.data_dir_lock.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                checkpoint_worktree_with_hooks(
                    Path::new(&root),
                    &reference,
                    elapsed,
                    limit,
                    lock.as_deref(),
                    hooks,
                )
            })
            .await;
            match outcome {
                Ok(Ok(checkpoint)) => checkpoint,
                Ok(Err(error)) => {
                    tracing::warn!(dispatch_job_id, "time-limit checkpoint failed: {error}");
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        dispatch_job_id,
                        "time-limit checkpoint task failed: {error}"
                    );
                    None
                }
            }
        }
        None => None,
    };
    let id = dispatch_job_id.to_string();
    if let Err(error) = state
        .db
        .with_conn(move |conn| {
            record_time_limit_stop(conn, &id, elapsed, limit, checkpoint.as_ref())
        })
        .await
    {
        tracing::warn!(dispatch_job_id, "time-limit stop not recorded: {error}");
    }
}

/// What the resume handoff says about the earlier generations: how long each
/// ran and what the limit left behind. `None` when no limit ever stopped this
/// execution.
pub fn handoff_note(conn: &Connection, execution_id: &str, fr: bool) -> Option<String> {
    let mut statement = conn
        .prepare(
            "SELECT changes_json FROM task_execution_events \
             WHERE task_execution_id = ?1 AND action = ?2 ORDER BY created_at, rowid",
        )
        .ok()?;
    let stops: Vec<Value> = statement
        .query_map(rusqlite::params![execution_id, TIME_LIMIT_EVENT], |row| {
            row.get::<_, String>(0)
        })
        .ok()?
        .filter_map(|row| row.ok())
        .filter_map(|text| serde_json::from_str(&text).ok())
        .collect();
    if stops.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for stop in &stops {
        let generation = stop["generation"].as_i64().unwrap_or(0);
        let elapsed = stop["elapsed_secs"].as_u64().unwrap_or(0) / 60;
        let limit = stop["limit_secs"].as_u64().unwrap_or(0) / 60;
        let count = stop["files"].as_array().map_or(0, Vec::len);
        let outcome = match (
            stop["checkpoint_commit"].as_str(),
            stop["patch"].as_str().is_some(),
            count,
        ) {
            (Some(commit), _, count) => {
                if fr {
                    format!(
                    "ses {count} fichier(s) non commités ont été sauvegardés dans le commit \
                     WIP `{commit}` de la branche de la tâche (message « {CHECKPOINT_SUBJECT_PREFIX} »)"
                )
                } else {
                    format!(
                        "its {count} uncommitted file(s) were saved in WIP commit `{commit}` on \
                     the task branch (message \"{CHECKPOINT_SUBJECT_PREFIX}\")"
                    )
                }
            }
            (None, true, count) => {
                if fr {
                    format!(
                        "aucun commit n'a pu être fait ; {count} fichier(s) non commités sont \
                     restés dans le worktree et un patch est conservé dans les preuves de \
                     l'exécution (événement `{TIME_LIMIT_EVENT}`)"
                    )
                } else {
                    format!(
                        "no commit could be made; {count} uncommitted file(s) stayed in the \
                     worktree and a patch is kept in the execution evidence (`{TIME_LIMIT_EVENT}` \
                     event)"
                    )
                }
            }
            (None, false, 0) => {
                if fr {
                    "le worktree était propre".to_string()
                } else {
                    "the worktree was clean".to_string()
                }
            }
            (None, false, count) => {
                if fr {
                    format!("{count} fichier(s) non commités sont restés dans le worktree")
                } else {
                    format!("{count} uncommitted file(s) stayed in the worktree")
                }
            }
        };
        lines.push(if fr {
            format!(
                "- génération {generation} : {elapsed} min sur {limit} min de limite, arrêtée par \
                 la limite ; {outcome}."
            )
        } else {
            format!(
                "- generation {generation}: {elapsed} min of the {limit}-min limit, stopped by \
                 the limit; {outcome}."
            )
        });
    }
    let list = lines.join("\n");
    Some(if fr {
        format!(
            "**Limite de temps** — les générations précédentes ont été arrêtées par la limite \
             d'exécution :\n{list}\n\
             Relis ce travail (`git show` du commit WIP, ou `git diff`), poursuis-le sans \
             réécrire l'historique, commite avec l'outil de commit déclaré et livre avec \
             `task_exec_deliver` bien avant la limite."
        )
    } else {
        format!(
            "**Time limit** — earlier generations were stopped by the execution limit:\n{list}\n\
             Review that work (`git show` of the WIP commit, or `git diff`), carry on without \
             rewriting history, commit with the declared commit tool and deliver with \
             `task_exec_deliver` well before the limit."
        )
    })
}

#[cfg(test)]
#[path = "worker_time_limit_tests.rs"]
mod tests;
