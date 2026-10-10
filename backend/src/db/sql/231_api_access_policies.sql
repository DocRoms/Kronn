-- KT-1026: which agents may call a plugin and which of its endpoints. No row
-- keeps the plugin as before; a row switches the broker to strict mode.
CREATE TABLE IF NOT EXISTS api_access_policies (
    server_id   TEXT PRIMARY KEY,
    policy_json TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
