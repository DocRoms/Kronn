-- KT-911 — the CLI sessions a task worker ran in, attempts included.
--
-- A worker's transcripts used to be found by the name of its worktree, which
-- says nothing about a rework, a relaunch or a reassigned worker. One row per
-- CLI process the worker started, keyed by the dispatch that launched it: a
-- rework has a new dispatch, a retried dispatch starts a new process, and both
-- keep their earlier rows.
--
-- `session_id` is the id the CLI reported on its init line, which is the name of
-- the transcript it wrote. `cost_usd` is NULL when unknown, never 0, and
-- `cost_unknown_reason` says why (KT-894).
CREATE TABLE task_execution_worker_sessions (
    dispatch_job_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_execution_id TEXT NOT NULL
        REFERENCES task_executions(id) ON DELETE CASCADE,
    attempt_no INTEGER NOT NULL,
    agent_type TEXT NOT NULL,
    cost_usd REAL,
    cost_unknown_reason TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (dispatch_job_id, session_id)
);

CREATE INDEX idx_task_execution_worker_sessions_execution
    ON task_execution_worker_sessions (task_execution_id, created_at);
