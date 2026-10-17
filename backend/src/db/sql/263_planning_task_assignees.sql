-- 0.15.1 (KT-154) — who is on which task.
--
-- A task can carry several assignees (an owner plus a reviewer, …). The
-- identity is the same typed regime as `message_targets` (KT-116): a
-- configured discussion agent, a punctual agent and an exact joined CLI session
-- are three different things even when all of them are Codex. A `cli` assignee
-- therefore pins the durable `discussion_sessions` row, never the provider
-- alone, so two Codex CLIs stay distinguishable (the KT-127 problem).
--
-- The role is optional: NULL means "on it, role unspecified".
--
-- This table only holds MANUAL assignments (human or agent made). The worker of
-- a running TaskExecution is read from `task_executions` at projection time and
-- is never copied here, so it cannot go stale when the execution ends.
CREATE TABLE planning_task_assignees (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id         TEXT NOT NULL REFERENCES planning_tasks(id) ON DELETE CASCADE,
    target_kind     TEXT NOT NULL
        CHECK (target_kind IN ('discussion_agent', 'agent', 'cli')),
    agent_type      TEXT NOT NULL,
    connection_id   TEXT,
    -- A cli assignee is deleted with its session row: an assignment to a
    -- session that no longer exists is not an assignment.
    cli_session_id  INTEGER REFERENCES discussion_sessions(id) ON DELETE CASCADE,
    role            TEXT CHECK (role IS NULL OR role IN ('owner', 'reviewer')),
    created_at      TEXT NOT NULL,
    CHECK ((target_kind = 'cli') = (cli_session_id IS NOT NULL))
);

CREATE UNIQUE INDEX idx_planning_task_assignees_identity
    ON planning_task_assignees(
        task_id,
        target_kind,
        agent_type,
        COALESCE(connection_id, ''),
        COALESCE(cli_session_id, -1)
    );

CREATE INDEX idx_planning_task_assignees_task
    ON planning_task_assignees(task_id, id);
