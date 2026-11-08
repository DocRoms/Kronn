-- KT-1564 — a provider fallback on quota exhaustion is its own, explicit
-- campaign opt-in. `allowed_agents` only says which providers a worker may be
-- given; it never meant "switch to them when the quota is spent", and reading
-- it that way moved executions to another account without anyone asking.
ALTER TABLE orchestration_runs
    ADD COLUMN quota_fallback INTEGER NOT NULL DEFAULT 0;

-- The alternatives a single-task launch names for its own fallback. Empty: a
-- campaign falls back among its allow-list, a launch that named none never
-- falls back. Kept apart from allowed_agents, which also restricts who a
-- person may reassign the execution to.
ALTER TABLE orchestration_runs
    ADD COLUMN quota_fallback_agents_json TEXT NOT NULL DEFAULT '[]';

-- The worker a quota fallback moved the execution away from, and the instant
-- the refusal announced its quota would be back. The watchdog hands the
-- execution back to that exact worker (provider, tier, model, profile) once
-- the instant has passed. One row per execution; a human reassignment, or the
-- return itself, deletes it.
CREATE TABLE IF NOT EXISTS task_execution_quota_origin (
    task_execution_id TEXT PRIMARY KEY
                        REFERENCES task_executions(id) ON DELETE CASCADE,
    origin_json       TEXT NOT NULL,
    origin_provider   TEXT NOT NULL,
    reset_at          TEXT,
    created_at        TEXT NOT NULL
);
