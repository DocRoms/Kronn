CREATE TABLE acp_runtime_sessions (
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    agent_type TEXT NOT NULL,
    runtime TEXT NOT NULL,
    project_scope TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (discussion_id, agent_type, runtime)
);

CREATE INDEX idx_acp_runtime_sessions_discussion
    ON acp_runtime_sessions(discussion_id, updated_at DESC);
