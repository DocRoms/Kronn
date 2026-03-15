-- Add include_general flag to MCP configs.
-- When true, this MCP is available in general discussions (no project).
-- Defaults to true so existing MCPs are automatically available.
ALTER TABLE mcp_configs ADD COLUMN include_general INTEGER NOT NULL DEFAULT 1;
