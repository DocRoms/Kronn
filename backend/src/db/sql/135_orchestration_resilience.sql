-- 0.11.0 (KT-322) — durable recovery, reassignment and cleanup policy.
--
-- Recovery is deliberately separate from `task_executions`: the state machine
-- remains the business source of truth, while this row records what the boot
-- reconciler observed and what a guarded resume is allowed to do next.

CREATE TABLE IF NOT EXISTS orchestration_run_resilience_policy (
    orchestration_run_id       TEXT PRIMARY KEY
                                   REFERENCES orchestration_runs(id) ON DELETE CASCADE,
    activity_timeout_secs      INTEGER CHECK (activity_timeout_secs IS NULL OR activity_timeout_secs > 0),
    review_timeout_secs        INTEGER CHECK (review_timeout_secs IS NULL OR review_timeout_secs > 0),
    human_wait_timeout_secs    INTEGER CHECK (human_wait_timeout_secs IS NULL OR human_wait_timeout_secs > 0),
    cancellation_cleanup_policy TEXT NOT NULL DEFAULT 'preserve'
                                   CHECK (cancellation_cleanup_policy IN ('preserve', 'remove_if_clean')),
    created_at                 TEXT NOT NULL,
    updated_at                 TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_execution_recovery (
    task_execution_id          TEXT PRIMARY KEY
                                   REFERENCES task_executions(id) ON DELETE CASCADE,
    recovery_action            TEXT NOT NULL,
    recovery_reason            TEXT NOT NULL,
    last_activity_at           TEXT NOT NULL,
    total_deadline_at          TEXT,
    activity_deadline_at       TEXT,
    review_deadline_at         TEXT,
    human_wait_started_at      TEXT,
    assignment_generation      INTEGER NOT NULL DEFAULT 0 CHECK (assignment_generation >= 0),
    pending                    INTEGER NOT NULL DEFAULT 1 CHECK (pending IN (0, 1)),
    updated_at                 TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_execution_recovery_deadlines
    ON task_execution_recovery(activity_deadline_at, review_deadline_at, total_deadline_at);

-- Every reassignment snapshots both the provider/runtime and the exact worker
-- identity. They are intentionally distinct: `claude_code` is not the same fact
-- as "CLI session 42", and replacing one must never erase the other.
CREATE TABLE IF NOT EXISTS task_execution_assignment_events (
    id                         TEXT PRIMARY KEY,
    task_execution_id          TEXT NOT NULL
                                   REFERENCES task_executions(id) ON DELETE CASCADE,
    generation                 INTEGER NOT NULL CHECK (generation >= 0),
    worker_target_kind         TEXT NOT NULL,
    worker_cli_session_id      INTEGER,
    worker_agent_type          TEXT NOT NULL,
    worker_model               TEXT,
    worker_model_tier          TEXT,
    worker_profile_id          TEXT,
    reason                     TEXT NOT NULL,
    actor_kind                 TEXT NOT NULL
                                   CHECK (actor_kind IN ('human', 'agent', 'backend', 'system')),
    actor_id                   TEXT,
    source_message_id          TEXT REFERENCES messages(id) ON DELETE SET NULL,
    created_at                 TEXT NOT NULL,
    UNIQUE(task_execution_id, generation)
);

-- A generic boot/cancellation journal also covers managed workspace orphans:
-- their FK was already SET NULL, so the event must retain opaque subject ids
-- after the row is safely removed.
CREATE TABLE IF NOT EXISTS orchestration_reconciliation_events (
    id                         TEXT PRIMARY KEY,
    subject_kind               TEXT NOT NULL
                                   CHECK (subject_kind IN ('execution', 'workspace', 'run')),
    subject_id                 TEXT NOT NULL,
    action                     TEXT NOT NULL,
    details_json               TEXT NOT NULL DEFAULT '{}',
    created_at                 TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_orchestration_reconciliation_subject
    ON orchestration_reconciliation_events(subject_kind, subject_id, created_at);
