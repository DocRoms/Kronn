-- KT-335 follow-up to 137. Migration 137 was already consumed by a running
-- development backend before dispatch failure/watchdog state was added; never
-- mutate an applied migration to smuggle these columns into existing DBs.
--
-- A provider quota and a dead process are operationally different failures.
-- Keep the ordinary dispatch status machine stable while making the cause and
-- the single watchdog retry durable and queryable.
ALTER TABLE agent_dispatch_jobs ADD COLUMN failure_kind TEXT;
ALTER TABLE agent_dispatch_jobs ADD COLUMN watchdog_redispatches INTEGER NOT NULL DEFAULT 0
    CHECK (watchdog_redispatches BETWEEN 0 AND 1);
ALTER TABLE task_execution_recovery ADD COLUMN watchdog_redispatches INTEGER NOT NULL DEFAULT 0
    CHECK (watchdog_redispatches BETWEEN 0 AND 1);

CREATE INDEX IF NOT EXISTS idx_agent_dispatch_watchdog
ON agent_dispatch_jobs(status, agent_started_at, watchdog_redispatches);
