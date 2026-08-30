-- Persist the last per-project MCP synchronization outcome so the UI can
-- explain whether files were written, already current, or could not be
-- updated. Nullable columns preserve the pre-migration "never synced" state.
ALTER TABLE projects ADD COLUMN mcp_sync_status TEXT;
ALTER TABLE projects ADD COLUMN mcp_sync_detail TEXT;
ALTER TABLE projects ADD COLUMN mcp_synced_at TEXT;
