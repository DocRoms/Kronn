-- Track cost per message (real from Claude Code, estimated for other providers)
ALTER TABLE messages ADD COLUMN cost_usd REAL DEFAULT NULL;
