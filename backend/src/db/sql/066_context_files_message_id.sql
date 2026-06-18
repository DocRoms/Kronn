-- 0.8.8: link an uploaded context file to the specific MESSAGE that carries it.
--
-- Before this, context_files only had a `discussion_id` FK, so an attached
-- image was "sticky" on the whole discussion: it never appeared in the sent
-- message bubble and stayed in the composer until deleted by hand.
--
-- `message_id` is nullable for back-compat:
--   * NULL          → pending (still staged in the composer) OR a legacy
--                     disc-wide file uploaded before this migration.
--   * non-NULL      → pinned to one message; rendered in that message's bubble.
-- On send, the backend flips every NULL row of the discussion to the new
-- user message id (see link_pending_context_files_to_message).
ALTER TABLE context_files ADD COLUMN message_id TEXT;
CREATE INDEX IF NOT EXISTS idx_context_files_message ON context_files(message_id);
