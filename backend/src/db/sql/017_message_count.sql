-- Add denormalized message_count column to avoid correlated subquery on every poll
ALTER TABLE discussions ADD COLUMN message_count INTEGER NOT NULL DEFAULT 0;

-- Backfill from existing data
UPDATE discussions SET message_count = (
    SELECT COUNT(*) FROM messages WHERE messages.discussion_id = discussions.id
);
