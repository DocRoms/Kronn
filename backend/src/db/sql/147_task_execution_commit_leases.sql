-- A spawned CLI worker commits through a server-side Git boundary.  Keep an
-- exact-dispatch lease while that non-transactional Git operation is in flight
-- so reassignment cannot replace the worker between authorization and commit.
CREATE TABLE IF NOT EXISTS task_execution_commit_leases (
    task_execution_id TEXT PRIMARY KEY
        REFERENCES task_executions(id) ON DELETE CASCADE,
    dispatch_job_id TEXT NOT NULL
        REFERENCES agent_dispatch_jobs(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
);
