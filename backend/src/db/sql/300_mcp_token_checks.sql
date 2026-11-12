-- KT-1692 — the proactive token check of an API connection (plugin config):
-- when its credentials were last probed (start, daily, on demand), what the
-- probe said, and when they last worked (a good probe or a good real call).
-- One row per config, written by the first probe or the first good call. The
-- refusal itself stays in `mcp_auth_failures` (KT-1524): a refused probe flags
-- the connection exactly like a refused call. `detail` is redacted before it
-- is stored.
CREATE TABLE IF NOT EXISTS mcp_token_checks (
    config_id    TEXT PRIMARY KEY REFERENCES mcp_configs(id) ON DELETE CASCADE,
    checked_at   TEXT,
    outcome      TEXT,
    detail       TEXT NOT NULL DEFAULT '',
    last_good_at TEXT
);
