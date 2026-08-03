ALTER TABLE messages
ADD COLUMN channel TEXT NOT NULL DEFAULT 'main'
CHECK (channel IN ('main', 'note'));

CREATE INDEX idx_messages_discussion_channel_sort
ON messages (discussion_id, channel, sort_order);
