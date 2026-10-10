-- KT-1111: a configuration assistant's conversation is kept, not deleted on
-- close, and linked to what it configures (a Custom API plugin, or an ApiCall
-- step of a workflow or Quick API). Deleting the discussion drops the link.
CREATE TABLE IF NOT EXISTS assistant_conversations (
    discussion_id           TEXT PRIMARY KEY REFERENCES discussions(id) ON DELETE CASCADE,
    kind                    TEXT NOT NULL CHECK (kind IN ('custom_api', 'api_call_step')),
    target_id               TEXT,
    target_step             TEXT,
    plugin_id               TEXT,
    target_label            TEXT NOT NULL DEFAULT '',
    last_proposal_signature TEXT,
    last_applied_signature  TEXT,
    last_applied_at         TEXT,
    created_at              TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_assistant_conversations_target
ON assistant_conversations(kind, target_id, target_step);
