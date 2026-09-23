-- a ceiling an agent run reached in a discussion, put to the human
-- as a question Kronn wrote itself. The row is what makes an answer count:
-- only a question recorded here can raise a budget, so an agent writing a
-- look-alike question in its own message cannot grant itself anything.
CREATE TABLE discussion_ceiling_requests (
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    question_key TEXT NOT NULL,
    -- The agent whose run hit the ceiling: a one-run grant is for its next run.
    agent_type TEXT NOT NULL,
    -- [{"kind":"tool","tool":"web_fetch","limit":120,"step":50},{"kind":"rounds","limit":150,"step":50}]
    ceilings_json TEXT NOT NULL,
    decision TEXT CHECK (decision IS NULL OR decision IN ('grant', 'unlimited', 'stop')),
    decided_at TEXT,
    -- A one-run grant is spent by the first run that reads it.
    consumed_at TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (discussion_id, question_key)
);
