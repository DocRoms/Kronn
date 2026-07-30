-- Preserve the exact joined CLI session that authored a live MCP message.
--
-- `messages.agent_type` identifies only the provider (Codex, ClaudeCode, ...).
-- It cannot distinguish two joined CLIs of the same provider, so a reply_to
-- needs this local provenance to route back to the exact author session.
CREATE TABLE message_cli_authors (
    message_id TEXT PRIMARY KEY
        REFERENCES messages(id) ON DELETE CASCADE,
    cli_session_id INTEGER NOT NULL
        REFERENCES discussion_sessions(id) ON DELETE CASCADE
);

CREATE INDEX idx_message_cli_authors_session
    ON message_cli_authors(cli_session_id, message_id);
