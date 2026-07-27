ALTER TABLE mcp_configs
ADD COLUMN preferred_interface TEXT NOT NULL DEFAULT 'mcp'
CHECK (preferred_interface IN ('api', 'mcp', 'cli'));

UPDATE mcp_configs
SET preferred_interface = 'api'
WHERE server_id IN (
    SELECT id
    FROM mcp_servers
    WHERE api_spec_json IS NOT NULL
);
