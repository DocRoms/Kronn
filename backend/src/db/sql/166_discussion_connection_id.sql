-- KT-545 — persist the discussion's sticky named-connection target.
-- An ordinary reply with no explicit @mention resolves through
-- `canonical_targets`'s implicit discussion-agent routing, which previously
-- had no connection to carry: `MessageTarget::discussion_agent()` always
-- sets `connection_id = NULL`, so a discussion whose primary agent is a
-- named Custom/LiteLLM/NVIDIA connection could not dispatch a plain
-- continuation message. This column is the durable source that routing now
-- reads instead of implicitly dropping the connection.
ALTER TABLE discussions
    ADD COLUMN connection_id TEXT
        REFERENCES external_api_connections(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_discussions_connection
    ON discussions(connection_id);
