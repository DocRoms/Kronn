-- KT-829 — the last probe result for each access a plugin config exposes
-- ("api" / "mcp" / "cli"), so the config list can show health without
-- re-running a probe on every page load. One row per (config, access);
-- a new probe simply overwrites the previous result for that pair.
CREATE TABLE IF NOT EXISTS mcp_probe_results (
    config_id   TEXT NOT NULL REFERENCES mcp_configs(id) ON DELETE CASCADE,
    access      TEXT NOT NULL,
    ok          INTEGER NOT NULL,
    code        TEXT NOT NULL,
    summary     TEXT NOT NULL,
    tested_at   TEXT NOT NULL,
    PRIMARY KEY (config_id, access)
);
