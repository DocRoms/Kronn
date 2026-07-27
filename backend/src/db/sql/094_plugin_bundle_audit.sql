-- 0.9.2 (KT-53) — durable, secret-free audit trail for plugin bundles.
--
-- Never store passphrases, ciphertext, plaintext, or environment values here.
-- The event row records only the selected config ids and outcome metadata.
CREATE TABLE IF NOT EXISTS plugin_bundle_events (
    id              TEXT PRIMARY KEY,
    action          TEXT NOT NULL CHECK (action IN ('export', 'import')),
    bundle_id       TEXT NOT NULL,
    config_ids_json TEXT NOT NULL DEFAULT '[]',
    includes_values INTEGER NOT NULL DEFAULT 0,
    success         INTEGER NOT NULL DEFAULT 1,
    detail_json     TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_plugin_bundle_events_created
    ON plugin_bundle_events(created_at DESC);

CREATE TABLE IF NOT EXISTS plugin_bundle_imports (
    source_bundle_id TEXT PRIMARY KEY,
    content_sha256   TEXT NOT NULL,
    report_json      TEXT NOT NULL,
    imported_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
