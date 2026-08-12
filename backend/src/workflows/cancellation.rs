//! Shared workflow-run cancellation.
//!
//! A workflow may be stopped by the HTTP operator action, by the global
//! workflow guard, or by a BatchQuickPrompt active-time budget. All paths must
//! cancel the same process tokens and settle the same durable dispatch rows.

use anyhow::{Context, Result};

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationScope {
    /// Cancel the addressed run as well as its child batches.
    RunTree,
    /// Keep the addressed linear run owned by its runner, but stop and settle
    /// every child batch. Used when the runner will persist its own terminal
    /// `StoppedByGuard`/`Cancelled` state immediately afterwards.
    DescendantsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancellationOutcome {
    pub run_cancelled: bool,
    pub child_discs_cancelled: u32,
    pub child_dispatches_settled: u32,
}

pub async fn cancel_run_tree(
    state: &AppState,
    run_id: &str,
    scope: CancellationScope,
    reason: &str,
) -> Result<CancellationOutcome> {
    let run_cancelled_in_memory = if scope == CancellationScope::RunTree {
        cancel_registered_token(state, run_id)?
    } else {
        false
    };

    let lookup_id = run_id.to_string();
    let child_disc_ids = state
        .db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT d.id FROM discussions d
                 JOIN workflow_runs wr ON d.workflow_run_id = wr.id
                 WHERE wr.parent_run_id = ?1 OR d.workflow_run_id = ?1",
            )?;
            let rows = stmt.query_map([lookup_id], |row| row.get::<_, String>(0))?;
            Ok(rows.filter_map(|row| row.ok()).collect::<Vec<_>>())
        })
        .await
        .context("find child discussions for workflow cancellation")?;

    let mut child_discs_cancelled = 0u32;
    for discussion_id in &child_disc_ids {
        child_discs_cancelled += u32::from(cancel_registered_token(state, discussion_id)?);
    }

    let settle_ids = child_disc_ids.clone();
    let settle_run_id = run_id.to_string();
    let settle_reason = reason.to_string();
    let (forced_parent, child_dispatches_settled) = state
        .db
        .with_conn(move |conn| {
            let transaction = conn.unchecked_transaction()?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut settled = 0u32;
            for discussion_id in &settle_ids {
                settled = settled.saturating_add(transaction.execute(
                    "UPDATE agent_dispatch_jobs
                     SET status = 'Cancelled', completed_at = ?2,
                         updated_at = ?2, last_error = ?3
                     WHERE discussion_id = ?1 AND status IN ('Pending', 'Running')",
                    rusqlite::params![discussion_id, now, settle_reason],
                )? as u32);
                crate::db::discussions::set_awaiting_agent(&transaction, discussion_id, false)?;
            }

            let forced_parent = if scope == CancellationScope::RunTree {
                transaction.execute(
                    "UPDATE workflow_runs
                     SET status = 'Cancelled', finished_at = ?2
                     WHERE id = ?1
                       AND status IN ('Running', 'Pending', 'WaitingApproval')",
                    rusqlite::params![settle_run_id, now],
                )?
            } else {
                0
            };
            transaction.execute(
                "UPDATE workflow_runs
                 SET status = 'Cancelled', finished_at = ?2
                 WHERE parent_run_id = ?1
                   AND status IN ('Running', 'Pending', 'WaitingApproval')",
                rusqlite::params![settle_run_id, now],
            )?;
            transaction.commit()?;
            Ok((forced_parent, settled))
        })
        .await
        .context("settle workflow cancellation")?;

    state.agent_dispatch_notify.notify_waiters();
    Ok(CancellationOutcome {
        run_cancelled: run_cancelled_in_memory || forced_parent > 0,
        child_discs_cancelled,
        child_dispatches_settled,
    })
}

fn cancel_registered_token(state: &AppState, key: &str) -> Result<bool> {
    let token = state
        .cancel_registry
        .lock()
        .map_err(|_| anyhow::anyhow!("cancel registry poisoned"))?
        .remove(key);
    if let Some(token) = token {
        token.cancel();
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn cancelling_batch_settles_all_eight_children_and_stops_live_tokens() {
        let db = Arc::new(crate::db::Database::open_in_memory().expect("in-memory DB"));
        let state = AppState::new_defaults(
            Arc::new(tokio::sync::RwLock::new(
                crate::core::config::default_config(),
            )),
            db,
            crate::DEFAULT_MAX_CONCURRENT_AGENTS,
        );
        let run_id = "batch-eight";
        let now = chrono::Utc::now();
        let run_id_for_seed = run_id.to_string();
        state
            .db
            .with_conn(move |conn| -> anyhow::Result<()> {
                conn.execute(
                    "INSERT INTO workflows
                     (id, name, trigger_json, steps_json, created_at, updated_at)
                     VALUES ('wf-cancel-eight', 'Cancel eight', '{}', '[]', ?1, ?1)",
                    [now.to_rfc3339()],
                )?;
                conn.execute(
                    "INSERT INTO workflow_runs
                     (id, workflow_id, run_type, status, started_at, batch_total)
                     VALUES (?1, 'wf-cancel-eight', 'batch', 'Running', ?2, 8)",
                    rusqlite::params![run_id_for_seed, now.to_rfc3339()],
                )?;
                for index in 0..8 {
                    let discussion_id = format!("cancel-child-{index}");
                    let message_id = format!("cancel-trigger-{index}");
                    conn.execute(
                        "INSERT INTO discussions
                         (id, title, agent, participants_json, workflow_run_id,
                          awaiting_agent, created_at, updated_at,
                          message_count, next_message_seq)
                         VALUES (?1, ?2, 'ClaudeCode', '[\"ClaudeCode\"]', ?3,
                                 1, ?4, ?4, 1, 1)",
                        rusqlite::params![
                            discussion_id,
                            format!("Child {index}"),
                            run_id_for_seed,
                            now.to_rfc3339(),
                        ],
                    )?;
                    conn.execute(
                        "INSERT INTO messages
                         (id, discussion_id, role, channel, content, timestamp, sort_order)
                         VALUES (?1, ?2, 'User', 'main', 'work', ?3, 0)",
                        rusqlite::params![message_id, discussion_id, now.to_rfc3339()],
                    )?;
                    crate::db::agent_dispatch::enqueue(
                        conn,
                        crate::db::agent_dispatch::NewAgentDispatchJob {
                            id: &format!("cancel-job-{index}"),
                            discussion_id: &discussion_id,
                            trigger_message_id: &message_id,
                            trigger_sort_order: 0,
                            dedupe_key: &format!("cancel-eight-{index}"),
                            agent_override: None,
                            chain_prompt_ids: &[],
                            batch_item: None,
                            group_id: Some(&run_id_for_seed),
                            group_concurrency_limit: Some(8),
                        },
                    )?;
                }
                Ok(())
            })
            .await
            .unwrap();

        let mut live_tokens = Vec::new();
        {
            let mut registry = state.cancel_registry.lock().unwrap();
            for index in 0..4 {
                let token = tokio_util::sync::CancellationToken::new();
                registry.insert(format!("cancel-child-{index}"), token.clone());
                live_tokens.push(token);
            }
        }

        let outcome = cancel_run_tree(
            &state,
            run_id,
            CancellationScope::RunTree,
            "LATENCY_BUDGET_EXCEEDED",
        )
        .await
        .unwrap();

        assert_eq!(outcome.child_discs_cancelled, 4);
        assert_eq!(outcome.child_dispatches_settled, 8);
        assert!(outcome.run_cancelled);
        assert!(live_tokens.iter().all(|token| token.is_cancelled()));

        state
            .db
            .with_conn(|conn| -> anyhow::Result<()> {
                let active: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM agent_dispatch_jobs
                     WHERE group_id = 'batch-eight' AND status IN ('Pending', 'Running')",
                    [],
                    |row| row.get(0),
                )?;
                let cancelled: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM agent_dispatch_jobs
                     WHERE group_id = 'batch-eight'
                       AND status = 'Cancelled'
                       AND completed_at IS NOT NULL
                       AND last_error = 'LATENCY_BUDGET_EXCEEDED'",
                    [],
                    |row| row.get(0),
                )?;
                let awaiting: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM discussions
                     WHERE workflow_run_id = 'batch-eight' AND awaiting_agent = 1",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(active, 0, "no child may continue consuming");
                assert_eq!(cancelled, 8, "every obligation is durably settled");
                assert_eq!(awaiting, 0, "no child remains visually pending");
                Ok(())
            })
            .await
            .unwrap();
    }
}
