-- Resync discussions.next_message_seq where it fell behind the real
-- MAX(messages.sort_order).
--
-- A stale counter made db::discussions::insert_message re-allocate an already
-- used sort_order, hitting the UNIQUE(discussion_id, sort_order) index added by
-- 082_message_sequence. insert_message then failed, which silently blocked
-- disc_append on the affected discussions (cron digests stopped posting).
--
-- The allocator is now self-healing (it takes MAX(counter, MAX(sort_order)+1)),
-- so this only needs to repair rows that were already stuck. Idempotent:
-- re-running it is a no-op once every counter is at or ahead of the max.
UPDATE discussions
SET next_message_seq = (
    SELECT MAX(m.sort_order) + 1 FROM messages m WHERE m.discussion_id = discussions.id
)
WHERE EXISTS (SELECT 1 FROM messages m WHERE m.discussion_id = discussions.id)
  AND next_message_seq < (
    SELECT MAX(m.sort_order) + 1 FROM messages m WHERE m.discussion_id = discussions.id
  );
