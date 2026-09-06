CREATE TABLE discussion_questions (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    fence_index INTEGER NOT NULL,
    question_key TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending', 'answered')),
    answer_json TEXT,
    answer_idempotency_key TEXT,
    requester_connection_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(discussion_id, question_key),
    UNIQUE(source_message_id, fence_index)
);
CREATE INDEX idx_discussion_questions_pending ON discussion_questions(discussion_id, state, created_at);
