-- Token tracking for discussion messages
ALTER TABLE messages ADD COLUMN tokens_used INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN auth_mode TEXT; -- 'override' or 'local'
