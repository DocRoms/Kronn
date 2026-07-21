-- Authenticated discussion-session resume across MCP bridge reloads.
-- Plain resume credentials are returned once and persisted only by the local
-- bridge; the database stores SHA-256 hashes. Rotation makes replay fail.
ALTER TABLE discussion_sessions ADD COLUMN resume_token_hash TEXT;
ALTER TABLE discussion_sessions ADD COLUMN resume_rotated_at DATETIME;

CREATE UNIQUE INDEX idx_disc_sessions_resume_token
    ON discussion_sessions(resume_token_hash)
    WHERE resume_token_hash IS NOT NULL;

-- Pre-077 host-launched identities were derived from the direct MCP parent
-- process. That PPID changes on reload, so those rows cannot be reclaimed
-- securely. Retire them once at upgrade rather than leave phantom participants;
-- the next explicit join receives a resumable credential.
UPDATE discussion_sessions
SET status = 'left', left_at = COALESCE(left_at, datetime('now'))
WHERE status != 'left'
  AND resume_token_hash IS NULL
  AND session_id LIKE 'adhoc-%';
