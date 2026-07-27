ALTER TABLE messages ADD COLUMN reply_to_message_id TEXT
    REFERENCES messages(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_messages_reply_to
    ON messages(reply_to_message_id)
    WHERE reply_to_message_id IS NOT NULL;
