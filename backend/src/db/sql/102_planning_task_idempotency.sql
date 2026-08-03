-- Durable idempotency for direct Planning task creation.
-- Keys are caller-scoped opaque values; titles remain ordinary task content.

ALTER TABLE planning_tasks ADD COLUMN idempotency_key TEXT;
ALTER TABLE planning_tasks ADD COLUMN idempotency_fingerprint TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_planning_tasks_idempotency_key
    ON planning_tasks(idempotency_key)
    WHERE idempotency_key IS NOT NULL;
