-- Named OpenAI-compatible API connections. Credentials remain in the encrypted
-- credential store and are referenced here by their stable slug.
CREATE TABLE external_api_connections (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    mention_alias TEXT NOT NULL UNIQUE,
    endpoint TEXT,
    credential_slug TEXT NOT NULL UNIQUE,
    origin_preset TEXT NOT NULL CHECK (origin_preset IN ('litellm', 'nvidia', 'other')),
    economy_model TEXT,
    default_model TEXT,
    reasoning_model TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_external_api_connections_origin_preset
    ON external_api_connections(origin_preset);
