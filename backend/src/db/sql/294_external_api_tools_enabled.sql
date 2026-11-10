-- Operator switch for the Kronn tool executor on runs on this external API
-- connection (KT-1600). NULL means "not switched off": tools stay on by
-- default, so existing connections keep them and nothing needs a backfill.
ALTER TABLE external_api_connections ADD COLUMN tools_enabled INTEGER;
