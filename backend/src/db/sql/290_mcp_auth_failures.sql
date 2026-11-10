-- KT-1524 — the last authentication refusal of an API connection (plugin
-- config): a 401/403 auth error, or a failed OAuth / token-exchange refresh.
-- One row per config while it is refused; the next successful call deletes it,
-- so a row means "this connection needs its token updated". `message` is the
-- provider's own (secret-scrubbed, truncated) answer.
CREATE TABLE IF NOT EXISTS mcp_auth_failures (
    config_id    TEXT PRIMARY KEY REFERENCES mcp_configs(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    http_status  INTEGER,
    message      TEXT NOT NULL,
    failed_at    TEXT NOT NULL
);
