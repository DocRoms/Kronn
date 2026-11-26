//! KT-1986 / KT-2021 — the durable record of a task worker's launch: which
//! dispatch started it, on which backend instance, and the process group of its
//! agent. The live handle on a worker (its cancel token, its `AgentProcess`)
//! dies with the backend; this row is what a boot, a resume, a reassignment and
//! the removal of the lane read to know a previous session may still run.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

/// One recorded worker launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerProcessRow {
    pub dispatch_job_id: String,
    pub execution_id: String,
    /// The backend instance that launched it (`core::dispatch_reaper::instance_id`).
    pub instance_id: String,
    /// The agent's process group, once it is spawned.
    pub agent_pgid: Option<i64>,
    /// The start stamp of that group's leader (`core::agent_orphans::start_stamp`).
    pub agent_started_marker: Option<String>,
    pub recorded_at: String,
    /// Set only by a stop that found nothing left of the session.
    pub stopped_at: Option<String>,
}

const COLUMNS: &str = "p.dispatch_job_id, p.execution_id, p.instance_id, p.agent_pgid, \
                       p.agent_started_marker, p.recorded_at, p.stopped_at";

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerProcessRow> {
    Ok(WorkerProcessRow {
        dispatch_job_id: row.get(0)?,
        execution_id: row.get(1)?,
        instance_id: row.get(2)?,
        agent_pgid: row.get(3)?,
        agent_started_marker: row.get(4)?,
        recorded_at: row.get(5)?,
        stopped_at: row.get(6)?,
    })
}

/// A dispatch is about to start a task worker. A dispatch launched again (a
/// watchdog requeue, a retry) opens its row again: the earlier agent is not
/// the one that will run.
pub fn record_launch(
    conn: &Connection,
    dispatch_job_id: &str,
    execution_id: &str,
    instance_id: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO task_worker_processes
             (dispatch_job_id, execution_id, instance_id, recorded_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(dispatch_job_id) DO UPDATE SET
             execution_id = excluded.execution_id,
             instance_id = excluded.instance_id,
             agent_pgid = NULL,
             agent_started_marker = NULL,
             recorded_at = excluded.recorded_at,
             stopped_at = NULL",
        params![
            dispatch_job_id,
            execution_id,
            instance_id,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// The agent of a recorded launch is spawned: its group and its start stamp.
/// `false` when the launch was never recorded.
pub fn set_agent(
    conn: &Connection,
    dispatch_job_id: &str,
    pgid: i64,
    marker: Option<&str>,
) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE task_worker_processes
            SET agent_pgid = ?2, agent_started_marker = ?3
          WHERE dispatch_job_id = ?1",
        params![dispatch_job_id, pgid, marker],
    )?;
    Ok(changed > 0)
}

pub fn get(conn: &Connection, dispatch_job_id: &str) -> Result<Option<WorkerProcessRow>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM task_worker_processes p WHERE p.dispatch_job_id = ?1"),
            params![dispatch_job_id],
            map_row,
        )
        .optional()?)
}

fn unsettled_where(
    conn: &Connection,
    from_where: &str,
    key: &str,
) -> Result<Vec<WorkerProcessRow>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM {from_where} AND p.stopped_at IS NULL \
         ORDER BY p.recorded_at, p.dispatch_job_id"
    ))?;
    let rows = statement
        .query_map(params![key], map_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The launches of an execution no stop has settled yet, on any instance.
pub fn unsettled_for_execution(
    conn: &Connection,
    execution_id: &str,
) -> Result<Vec<WorkerProcessRow>> {
    unsettled_where(
        conn,
        "task_worker_processes p WHERE p.execution_id = ?1",
        execution_id,
    )
}

/// The launches of one backend instance no stop has settled yet.
pub fn unsettled_for_instance(
    conn: &Connection,
    instance_id: &str,
) -> Result<Vec<WorkerProcessRow>> {
    unsettled_where(
        conn,
        "task_worker_processes p WHERE p.instance_id = ?1",
        instance_id,
    )
}

/// The unsettled launches whose dispatch ran in `discussion_id`: what is left
/// to look at once a checkout has lost its execution.
pub fn unsettled_for_discussion(
    conn: &Connection,
    discussion_id: &str,
) -> Result<Vec<WorkerProcessRow>> {
    unsettled_where(
        conn,
        "task_worker_processes p \
         JOIN agent_dispatch_jobs j ON j.id = p.dispatch_job_id \
         WHERE j.discussion_id = ?1",
        discussion_id,
    )
}

/// A stop found nothing left of the session. `false` when it was already
/// settled, or never recorded.
pub fn mark_stopped(conn: &Connection, dispatch_job_id: &str) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE task_worker_processes SET stopped_at = ?2
          WHERE dispatch_job_id = ?1 AND stopped_at IS NULL",
        params![dispatch_job_id, Utc::now().to_rfc3339()],
    )?;
    Ok(changed > 0)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        crate::db::migrations::run(&conn).unwrap();
        conn
    }

    /// A launch is recorded before its agent exists, completed with the agent's
    /// group and stamp, and leaves the unsettled lists once it is stopped.
    #[test]
    fn a_launch_is_recorded_then_completed_with_its_agent() {
        let conn = connection();
        assert!(get(&conn, "d-1").unwrap().is_none());
        assert!(
            !set_agent(&conn, "d-1", 4242, None).unwrap(),
            "no launch, no agent"
        );

        record_launch(&conn, "d-1", "exec-1", "inst-a").unwrap();
        let recorded = get(&conn, "d-1").unwrap().expect("the launch");
        assert_eq!(recorded.execution_id, "exec-1");
        assert_eq!(recorded.instance_id, "inst-a");
        assert_eq!(recorded.agent_pgid, None);
        assert_eq!(recorded.stopped_at, None);

        assert!(set_agent(&conn, "d-1", 4242, Some("bsd:1.2")).unwrap());
        let with_agent = get(&conn, "d-1").unwrap().unwrap();
        assert_eq!(with_agent.agent_pgid, Some(4242));
        assert_eq!(with_agent.agent_started_marker.as_deref(), Some("bsd:1.2"));
        assert_eq!(
            unsettled_for_execution(&conn, "exec-1").unwrap(),
            vec![with_agent]
        );

        assert!(mark_stopped(&conn, "d-1").unwrap());
        assert!(!mark_stopped(&conn, "d-1").unwrap(), "settled once");
        assert!(get(&conn, "d-1").unwrap().unwrap().stopped_at.is_some());
        assert!(unsettled_for_execution(&conn, "exec-1").unwrap().is_empty());
    }

    /// A dispatch launched a second time is unsettled again and forgets the
    /// agent of its first launch.
    #[test]
    fn a_relaunch_of_the_same_dispatch_reopens_its_row() {
        let conn = connection();
        record_launch(&conn, "d-1", "exec-1", "inst-a").unwrap();
        set_agent(&conn, "d-1", 4242, Some("bsd:1.2")).unwrap();
        mark_stopped(&conn, "d-1").unwrap();

        record_launch(&conn, "d-1", "exec-1", "inst-a").unwrap();

        let reopened = get(&conn, "d-1").unwrap().unwrap();
        assert_eq!(reopened.stopped_at, None);
        assert_eq!(reopened.agent_pgid, None);
        assert_eq!(reopened.agent_started_marker, None);
        assert_eq!(unsettled_for_instance(&conn, "inst-a").unwrap().len(), 1);
    }

    /// Boot reads the launches of its own instance that no stop settled: never
    /// another instance's, never a settled one.
    #[test]
    fn only_unsettled_rows_of_this_instance_are_listed() {
        let conn = connection();
        record_launch(&conn, "d-mine", "exec-1", "inst-a").unwrap();
        record_launch(&conn, "d-stopped", "exec-1", "inst-a").unwrap();
        record_launch(&conn, "d-foreign", "exec-1", "inst-b").unwrap();
        mark_stopped(&conn, "d-stopped").unwrap();

        let mine: Vec<String> = unsettled_for_instance(&conn, "inst-a")
            .unwrap()
            .into_iter()
            .map(|row| row.dispatch_job_id)
            .collect();
        assert_eq!(mine, ["d-mine"]);
        // The execution's own list spans instances: the caller tells them apart.
        let mut of_execution: Vec<String> = unsettled_for_execution(&conn, "exec-1")
            .unwrap()
            .into_iter()
            .map(|row| row.dispatch_job_id)
            .collect();
        of_execution.sort();
        assert_eq!(of_execution, ["d-foreign", "d-mine"]);
        assert!(unsettled_for_instance(&conn, "inst-c").unwrap().is_empty());
    }

    /// A launch is found from the room its dispatch ran in, for a checkout
    /// whose execution is gone.
    #[test]
    fn unsettled_rows_are_found_from_the_room_of_their_dispatch() {
        let conn = connection();
        conn.execute(
            "INSERT INTO discussions (id, title, created_at, updated_at)
             VALUES ('d-room', 'Room', '2026-11-26T00:00:00Z', '2026-11-26T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages
             (id, discussion_id, role, content, timestamp, sort_order, received_at)
             VALUES ('u-room', 'd-room', 'User', 'go', '2026-11-26T00:00:00Z', 1,
                     '2026-11-26T00:00:00Z')",
            [],
        )
        .unwrap();
        crate::db::agent_dispatch::enqueue_for_latest_user(
            &conn,
            crate::db::agent_dispatch::NewLatestUserDispatch {
                id: "j-room",
                discussion_id: "d-room",
                dedupe_key: "message:u-room",
                agent_override: Some(&crate::models::AgentType::Ollama),
                chain_prompt_ids: &[],
                batch_item: None,
                group_id: None,
                group_concurrency_limit: None,
            },
        )
        .unwrap();
        record_launch(&conn, "j-room", "exec-gone", "inst-a").unwrap();
        record_launch(&conn, "j-elsewhere", "exec-gone", "inst-a").unwrap();

        let of_room: Vec<String> = unsettled_for_discussion(&conn, "d-room")
            .unwrap()
            .into_iter()
            .map(|row| row.dispatch_job_id)
            .collect();
        assert_eq!(of_room, ["j-room"]);
        mark_stopped(&conn, "j-room").unwrap();
        assert!(unsettled_for_discussion(&conn, "d-room")
            .unwrap()
            .is_empty());
    }
}
