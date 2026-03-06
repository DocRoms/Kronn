-- ═══════════════════════════════════════════════════════════════════════════════
-- Kronn — MCP redesign: server → config → project model
-- ═══════════════════════════════════════════════════════════════════════════════

-- Drop legacy MCP table (clean slate)
DROP TABLE IF EXISTS mcps;
DROP INDEX IF EXISTS idx_mcps_project;

-- MCP server types (abstract definitions)
CREATE TABLE IF NOT EXISTS mcp_servers (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    transport   TEXT NOT NULL,          -- 'stdio' or 'sse' or 'streamable'
    command     TEXT,                   -- for stdio: binary name
    args_json   TEXT NOT NULL DEFAULT '[]',  -- JSON array of default args
    url         TEXT,                   -- for sse/streamable
    source      TEXT NOT NULL DEFAULT 'detected'  -- 'registry', 'detected', 'manual'
);

-- MCP configured instances (with label + encrypted env)
CREATE TABLE IF NOT EXISTS mcp_configs (
    id              TEXT PRIMARY KEY,
    server_id       TEXT NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    label           TEXT NOT NULL,
    env_encrypted   TEXT NOT NULL DEFAULT '',   -- AES-256-GCM encrypted JSON of env vars
    env_keys_json   TEXT NOT NULL DEFAULT '[]', -- JSON array of key names (for display)
    args_override   TEXT,                       -- JSON array, null = use server defaults
    is_global       INTEGER NOT NULL DEFAULT 0,
    config_hash     TEXT NOT NULL DEFAULT ''    -- SHA256(command+args+env_values) for dedup
);

CREATE INDEX IF NOT EXISTS idx_mcp_configs_server ON mcp_configs(server_id);
CREATE INDEX IF NOT EXISTS idx_mcp_configs_hash ON mcp_configs(config_hash);

-- N:N linkage between configs and projects
CREATE TABLE IF NOT EXISTS mcp_config_projects (
    config_id   TEXT NOT NULL REFERENCES mcp_configs(id) ON DELETE CASCADE,
    project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    PRIMARY KEY (config_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_mcp_cp_project ON mcp_config_projects(project_id);
