-- A human must be able to refuse an arbitration, not only answer it.
--
-- Until now `state` allowed 'pending' and 'answered' only, so the single way to
-- clear a question was to pick one of its options — including for a question
-- that should never have been asked. A run that died left its question pending
-- for good, and the banner could not be dismissed without inventing a decision.
--
-- SQLite cannot extend a CHECK constraint in place; rebuild the table
-- additively, retaining every row, index and foreign key.
CREATE TABLE discussion_questions_next (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    fence_index INTEGER NOT NULL,
    question_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK(state IN ('pending', 'answered', 'declined')),
    answer_json TEXT,
    answer_idempotency_key TEXT,
    requester_connection_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(discussion_id, question_key),
    UNIQUE(source_message_id, fence_index)
);
INSERT INTO discussion_questions_next
SELECT id, discussion_id, source_message_id, fence_index, question_key,
       payload_json, state, answer_json, answer_idempotency_key,
       requester_connection_id, created_at, updated_at
FROM discussion_questions;
DROP TABLE discussion_questions;
ALTER TABLE discussion_questions_next RENAME TO discussion_questions;
CREATE INDEX idx_discussion_questions_pending
    ON discussion_questions(discussion_id, state, created_at);
