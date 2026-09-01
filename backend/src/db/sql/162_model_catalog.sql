-- KT-531 (migration 162): dynamic model catalog. Canonical identity is
-- (runtime_target_id, model_id); agent_type is projection metadata only.
CREATE TABLE IF NOT EXISTS model_catalog_entries (
    id TEXT PRIMARY KEY,
    runtime_target_id TEXT NOT NULL,
    agent_type TEXT NOT NULL,
    model_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    display_alias TEXT,
    provenance TEXT NOT NULL CHECK(provenance IN ('live','cached','manual','migrated')),
    availability TEXT NOT NULL DEFAULT 'available' CHECK(availability IN ('available','unavailable')),
    unavailable_reason TEXT,
    unavailable_detail TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '[]',
    reasoning_modes_json TEXT NOT NULL DEFAULT '[]',
    default_reasoning_mode TEXT,
    tier_assignment TEXT CHECK(tier_assignment IS NULL OR tier_assignment IN ('economy','default','reasoning')),
    manual_origin INTEGER NOT NULL DEFAULT 0,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT,
    last_checked_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(runtime_target_id, model_id)
);

CREATE INDEX IF NOT EXISTS idx_model_catalog_runtime_target
    ON model_catalog_entries(runtime_target_id);
CREATE INDEX IF NOT EXISTS idx_model_catalog_agent_type
    ON model_catalog_entries(agent_type);

-- Refresh state belongs to a durable target, not an AgentType bucket. This
-- preserves independent catalog freshness for two HTTP connections exposing
-- the same provider model id.
CREATE TABLE IF NOT EXISTS model_catalog_refresh_log (
    runtime_target_id TEXT PRIMARY KEY,
    agent_type TEXT NOT NULL,
    last_live_success_at TEXT,
    last_attempt_at TEXT NOT NULL,
    last_error_reason TEXT,
    last_error_detail TEXT
);

CREATE TABLE IF NOT EXISTS model_catalog_migration_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    migrated_at TEXT NOT NULL
);
