CREATE TABLE IF NOT EXISTS discussion_actions (
    id TEXT PRIMARY KEY,
    discussion_id TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    source_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    fence_index INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('quick_prompt','quick_api','quick_exec','workflow','invalid')),
    target_id TEXT NOT NULL,
    target_name TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    state TEXT NOT NULL DEFAULT 'proposed' CHECK(state IN (
        'proposed','launching','running','succeeded','failed',
        'cancelled','preflight_failed'
    )),
    values_json TEXT NOT NULL DEFAULT '[]',
    shared_run_id TEXT REFERENCES shared_runs(id) ON DELETE SET NULL,
    result_discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    deep_link TEXT,
    diagnostic TEXT,
    launched_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(source_message_id, fence_index)
);

CREATE INDEX IF NOT EXISTS idx_discussion_actions_discussion
    ON discussion_actions(discussion_id, created_at, id);
CREATE INDEX IF NOT EXISTS idx_discussion_actions_message
    ON discussion_actions(source_message_id, fence_index);
CREATE INDEX IF NOT EXISTS idx_discussion_actions_state
    ON discussion_actions(state, updated_at);
