-- A human acknowledgement that a provider quota has been replenished.  This
-- deliberately leaves executions and their recovery evidence untouched.
CREATE TABLE IF NOT EXISTS provider_quota_rearms (
    provider TEXT PRIMARY KEY,
    rearmed_at TEXT NOT NULL,
    idempotency_key TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS provider_quota_rearm_events (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    idempotency_key TEXT NOT NULL UNIQUE,
    rearmed_at TEXT NOT NULL
);
