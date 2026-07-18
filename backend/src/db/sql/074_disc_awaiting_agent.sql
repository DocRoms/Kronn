-- 0.8.12 T1 — deterministic marker for "an agent run is owed on this disc
-- but has produced no durable trace yet". Set at enqueue (a batch child's
-- creation, or a human message that will spawn an auto-reply); cleared when
-- the agent delivers its response. A backend restart between enqueue and the
-- first partial checkpoint used to lose that work silently — this column lets
-- the boot reconcile find it (WHERE awaiting_agent=1 AND partial_response IS
-- NULL) and mark it interrupted instead of leaving a dead discussion.
-- Same spirit as partial_response (migration 031), NOT a job queue.
ALTER TABLE discussions ADD COLUMN awaiting_agent INTEGER NOT NULL DEFAULT 0;

-- Partial index so the boot scan is cheap even on a large discussions table.
CREATE INDEX IF NOT EXISTS idx_discussions_awaiting_agent
    ON discussions(awaiting_agent)
    WHERE awaiting_agent = 1;
