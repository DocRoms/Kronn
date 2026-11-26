-- KT-1986 / KT-2021 — the launch of a task worker by a dispatch, on this
-- backend instance (`instance_id`: a hash of its data directory), with the
-- process group of its agent and the start stamp of that group's leader once
-- the agent is spawned. A row means "this dispatch launched a task worker
-- here"; `stopped_at` is set only once a stop found nothing left of it. Boot,
-- a resume, a reassignment and the removal of the lane read the rows that are
-- not stopped. No foreign key: a row outlives the deletion of its dispatch or
-- of its execution, because the process may.
CREATE TABLE IF NOT EXISTS task_worker_processes (
    dispatch_job_id      TEXT PRIMARY KEY,
    execution_id         TEXT NOT NULL,
    instance_id          TEXT NOT NULL,
    agent_pgid           INTEGER,
    agent_started_marker TEXT,
    recorded_at          TEXT NOT NULL,
    stopped_at           TEXT
);
CREATE INDEX IF NOT EXISTS idx_task_worker_processes_execution
    ON task_worker_processes(execution_id);
CREATE INDEX IF NOT EXISTS idx_task_worker_processes_instance
    ON task_worker_processes(instance_id, stopped_at);
