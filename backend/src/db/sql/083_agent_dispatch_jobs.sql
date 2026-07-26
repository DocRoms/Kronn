ALTER TABLE messages
ADD COLUMN recovered_partial INTEGER NOT NULL DEFAULT 0;

ALTER TABLE messages
ADD COLUMN agent_run_succeeded INTEGER;

ALTER TABLE messages
ADD COLUMN agent_dispatch_job_id TEXT;

CREATE TABLE IF NOT EXISTS agent_dispatch_jobs (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    trigger_sort_order INTEGER NOT NULL,
    dedupe_key TEXT NOT NULL UNIQUE,
    agent_override_json TEXT,
    chain_prompt_ids_json TEXT NOT NULL DEFAULT '[]',
    next_chain_index INTEGER NOT NULL DEFAULT 0,
    batch_item TEXT,
    group_id TEXT,
    group_concurrency_limit INTEGER,
    status TEXT NOT NULL DEFAULT 'Pending'
        CHECK (status IN ('Pending', 'Running', 'Completed', 'Failed', 'Cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    turn_attempts INTEGER NOT NULL DEFAULT 0,
    available_at TEXT NOT NULL,
    claimed_at TEXT,
    completed_at TEXT,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_dispatch_one_running_discussion
ON agent_dispatch_jobs(discussion_id)
WHERE status = 'Running';

CREATE INDEX IF NOT EXISTS idx_agent_dispatch_runnable
ON agent_dispatch_jobs(status, available_at, created_at);

CREATE INDEX IF NOT EXISTS idx_agent_dispatch_group_status
ON agent_dispatch_jobs(group_id, status);
