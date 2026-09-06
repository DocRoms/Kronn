-- KT-593 follow-up: recovery.updated_at is generic housekeeping, not quota
-- evidence. Keep quota evidence on a dedicated monotonic provider generation.
ALTER TABLE task_execution_recovery
    ADD COLUMN quota_signal_generation INTEGER NOT NULL DEFAULT 0;

ALTER TABLE provider_quota_rearms
    ADD COLUMN acknowledged_generation INTEGER NOT NULL DEFAULT 0;

-- Existing quota markers predate the dedicated field. They remain one initial
-- generation; a historical re-arm therefore acknowledges that generation.
UPDATE task_execution_recovery
SET quota_signal_generation = 1
WHERE recovery_reason LIKE 'quota_exhausted:%';

UPDATE provider_quota_rearms
SET acknowledged_generation = 1;

CREATE TABLE provider_quota_generations (
    provider TEXT PRIMARY KEY,
    latest_generation INTEGER NOT NULL
);

INSERT INTO provider_quota_generations (provider, latest_generation)
SELECT substr(recovery_reason, length('quota_exhausted:') + 1),
       MAX(quota_signal_generation)
FROM task_execution_recovery
WHERE recovery_reason LIKE 'quota_exhausted:%'
GROUP BY substr(recovery_reason, length('quota_exhausted:') + 1);

-- Migration 169 originally made this key global. Rebuild it with the provider
-- in its uniqueness boundary so a replay for one provider cannot suppress a
-- different provider's acknowledgement.
CREATE TABLE provider_quota_rearm_events_v170 (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    actor_kind TEXT NOT NULL DEFAULT 'human',
    actor_id TEXT,
    rearmed_at TEXT NOT NULL,
    UNIQUE(provider, idempotency_key)
);

INSERT INTO provider_quota_rearm_events_v170
    (id, provider, idempotency_key, actor_kind, actor_id, rearmed_at)
SELECT id, provider, idempotency_key, 'human', NULL, rearmed_at
FROM provider_quota_rearm_events;

DROP TABLE provider_quota_rearm_events;
ALTER TABLE provider_quota_rearm_events_v170 RENAME TO provider_quota_rearm_events;
