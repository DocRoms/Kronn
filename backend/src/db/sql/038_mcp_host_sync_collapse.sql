-- Collapse `host_sync` mode `MirrorAll` into the orthogonal pair
-- `is_global = true + host_sync = 'GlobalOnly'`.
--
-- The 3-mode UI (None / GlobalOnly / MirrorAll) was found to be confusing
-- because `MirrorAll` overrode the user's project_ids selection, creating
-- two competing scope dimensions. The new model: `host_sync` is a binary
-- "synced to host CLIs?" flag, and `is_global` keeps its original
-- semantics ("applied to all Kronn projects"). On Claude Code, scope is
-- routed to `projects[<host-path>].mcpServers` when project_ids is set.
--
-- Step 1: configs that were `MirrorAll` had implicit "auto-apply to all
-- projects" semantics — preserve it explicitly via `is_global = 1`.
UPDATE mcp_configs SET is_global = 1 WHERE host_sync = 'MirrorAll';

-- Step 2: collapse `MirrorAll` rows to the canonical "synced" value.
-- The Rust enum still accepts `MirrorAll` as a defensive read fallback
-- (parse_host_sync) so a stale row from a downgrade is non-fatal — but
-- new writes use only `None` or `GlobalOnly`.
UPDATE mcp_configs SET host_sync = 'GlobalOnly' WHERE host_sync = 'MirrorAll';
