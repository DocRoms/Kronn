-- 0.11.0 (KT-321) — durable principal/campaign coordination policy.
--
-- Keep the original lifecycle `status` intact for backwards compatibility with
-- databases created by migration 127. `control_state` is the operator-facing
-- campaign state: a run may be durably paused or waiting for a human while its
-- coarse lifecycle remains active.

ALTER TABLE orchestration_runs
    ADD COLUMN control_state TEXT NOT NULL DEFAULT 'running'
        CHECK (control_state IN (
            'running', 'paused', 'awaiting_human',
            'completed', 'cancelled', 'failed'
        ));
ALTER TABLE orchestration_runs ADD COLUMN control_reason TEXT;
ALTER TABLE orchestration_runs ADD COLUMN timeout_secs INTEGER;
ALTER TABLE orchestration_runs
    ADD COLUMN max_cli_concurrent_executions INTEGER NOT NULL DEFAULT 1;
ALTER TABLE orchestration_runs
    ADD COLUMN allowed_agents_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE orchestration_runs ADD COLUMN default_worker_json TEXT;
ALTER TABLE orchestration_runs
    ADD COLUMN auto_continue INTEGER NOT NULL DEFAULT 0;

-- A profile is part of the worker choice just like the exact model. It is kept
-- on the execution audit row even if the profile is later edited or deleted.
ALTER TABLE task_executions ADD COLUMN worker_profile_id TEXT;

CREATE INDEX IF NOT EXISTS idx_task_executions_run_active
    ON task_executions(orchestration_run_id, status)
    WHERE status NOT IN ('Done', 'Failed', 'Cancelled');
CREATE INDEX IF NOT EXISTS idx_task_executions_cli_active
    ON task_executions(parent_discussion_id, worker_target_kind, status)
    WHERE worker_target_kind = 'cli'
      AND status NOT IN ('Done', 'Failed', 'Cancelled');

CREATE TABLE IF NOT EXISTS orchestration_run_events (
    id                  TEXT PRIMARY KEY,
    orchestration_run_id TEXT NOT NULL
                            REFERENCES orchestration_runs(id) ON DELETE CASCADE,
    action              TEXT NOT NULL,
    from_state          TEXT,
    to_state            TEXT,
    actor_kind          TEXT NOT NULL
                            CHECK (actor_kind IN ('human', 'agent', 'backend', 'system')),
    actor_id            TEXT,
    changes_json        TEXT NOT NULL DEFAULT '{}',
    source_message_id   TEXT REFERENCES messages(id) ON DELETE SET NULL,
    created_at          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_orchestration_run_events_run
    ON orchestration_run_events(orchestration_run_id, created_at);
