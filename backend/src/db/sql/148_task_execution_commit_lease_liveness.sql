ALTER TABLE task_execution_commit_leases
    RENAME TO task_execution_commit_leases_legacy;

CREATE TABLE task_execution_commit_leases (
    task_execution_id TEXT PRIMARY KEY
        REFERENCES task_executions(id) ON DELETE CASCADE,
    dispatch_job_id TEXT NOT NULL
        REFERENCES agent_dispatch_jobs(id) ON DELETE CASCADE,
    lease_id TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    -- Set only after the supervised Git process has returned successfully.
    -- A later reassignment may safely reap this row if deleting it failed.
    settled_at TEXT
);

DROP TABLE task_execution_commit_leases_legacy;
