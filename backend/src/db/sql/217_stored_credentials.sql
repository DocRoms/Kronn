-- KT-1007 — provider/connection keys and the API auth token, encrypted with the
-- instance key instead of sitting in plaintext in config.toml.
CREATE TABLE IF NOT EXISTS stored_credentials (
    kind TEXT NOT NULL CHECK (kind IN ('provider_key', 'auth_token')),
    id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    provider TEXT NOT NULL DEFAULT '',
    active INTEGER NOT NULL DEFAULT 0,
    position INTEGER NOT NULL DEFAULT 0,
    value_encrypted TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (kind, id)
);
