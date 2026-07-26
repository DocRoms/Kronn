ALTER TABLE discussions
ADD COLUMN next_message_seq INTEGER NOT NULL DEFAULT 1;

UPDATE discussions
SET next_message_seq = COALESCE(
    (
        SELECT MAX(messages.sort_order) + 1
        FROM messages
        WHERE messages.discussion_id = discussions.id
    ),
    1
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_messages_discussion_sort_order_unique
ON messages(discussion_id, sort_order);
