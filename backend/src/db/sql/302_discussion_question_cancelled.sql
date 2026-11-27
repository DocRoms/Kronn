-- KT-1955 — a human must be able to close a card without deciding it.
--
-- Answering and refusing both reach the agent and resume what was parked on
-- the card. A card answered another way (in a CLI, by mouth) or no longer
-- worth deciding kept the room's banner red for good. `cancelled` closes it
-- and nothing else: no message, no dispatch, no resume. Who cancelled and when
-- are kept on the row, since a cancelled card carries no answer.
--
-- SQLite cannot extend a CHECK constraint in place; rebuild the table
-- additively, retaining every row, column and index. No table references
-- discussion_questions, so the rebuild deletes nothing elsewhere.
CREATE TABLE discussion_questions_next (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    fence_index INTEGER NOT NULL,
    question_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK(state IN ('pending', 'answered', 'declined', 'cancelled')),
    answer_json TEXT,
    answer_idempotency_key TEXT,
    requester_connection_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    resume_state TEXT,
    resume_run_id TEXT,
    resume_error TEXT,
    cancelled_by TEXT,
    cancelled_at TEXT,
    UNIQUE(discussion_id, question_key),
    UNIQUE(source_message_id, fence_index)
);
INSERT INTO discussion_questions_next
    (id, discussion_id, source_message_id, fence_index, question_key,
     payload_json, state, answer_json, answer_idempotency_key,
     requester_connection_id, created_at, updated_at,
     resume_state, resume_run_id, resume_error)
SELECT id, discussion_id, source_message_id, fence_index, question_key,
       payload_json, state, answer_json, answer_idempotency_key,
       requester_connection_id, created_at, updated_at,
       resume_state, resume_run_id, resume_error
FROM discussion_questions;
DROP TABLE discussion_questions;
ALTER TABLE discussion_questions_next RENAME TO discussion_questions;
CREATE INDEX idx_discussion_questions_pending
    ON discussion_questions(discussion_id, state, created_at);
CREATE INDEX idx_discussion_questions_resume_run
    ON discussion_questions(resume_run_id) WHERE resume_run_id IS NOT NULL;
