-- KT-335 — backend-owned work and scheduled wakes that outlive the agent turn.
-- The command snapshot is a validated QuickExecSpec (literal argv, bounded cwd,
-- no shell); the completion dispatch is anchored to this durable row.
CREATE TABLE IF NOT EXISTS agent_resume_jobs (
    id                       TEXT PRIMARY KEY,
    discussion_id            TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    target_agent_json         TEXT NOT NULL,
    source_dispatch_job_id    TEXT REFERENCES agent_dispatch_jobs(id) ON DELETE SET NULL,
    task_execution_id         TEXT REFERENCES task_executions(id) ON DELETE SET NULL,
    quick_exec_id             TEXT REFERENCES quick_execs(id) ON DELETE SET NULL,
    kind                      TEXT NOT NULL CHECK (kind IN ('Command', 'Wake')),
    status                    TEXT NOT NULL DEFAULT 'Pending'
        CHECK (status IN ('Pending', 'Running', 'Completed', 'Failed',
                          'Cancelled', 'QuotaExhausted', 'Escalated')),
    dedupe_key                TEXT NOT NULL UNIQUE,
    reason                    TEXT NOT NULL,
    command_spec_json         TEXT,
    result_json               TEXT,
    failure_kind              TEXT,
    scheduled_at              TEXT NOT NULL,
    chain_depth               INTEGER NOT NULL DEFAULT 0 CHECK (chain_depth >= 0),
    wake_budget               INTEGER NOT NULL DEFAULT 3 CHECK (wake_budget BETWEEN 1 AND 10),
    watchdog_redispatches     INTEGER NOT NULL DEFAULT 0
        CHECK (watchdog_redispatches BETWEEN 0 AND 1),
    completion_dispatch_id    TEXT REFERENCES agent_dispatch_jobs(id) ON DELETE SET NULL,
    started_at                TEXT,
    completed_at              TEXT,
    last_error                TEXT,
    created_at                TEXT NOT NULL,
    updated_at                TEXT NOT NULL,
    CHECK ((kind = 'Command' AND command_spec_json IS NOT NULL)
        OR (kind = 'Wake' AND command_spec_json IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_agent_resume_jobs_runnable
ON agent_resume_jobs(status, scheduled_at, created_at);

CREATE INDEX IF NOT EXISTS idx_agent_resume_jobs_discussion_active
ON agent_resume_jobs(discussion_id, status, updated_at);
