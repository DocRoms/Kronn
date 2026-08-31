CREATE TABLE IF NOT EXISTS shared_runs (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('quick_prompt','quick_api','quick_exec','workflow')),
    source_id TEXT NOT NULL,
    project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
    discussion_id TEXT REFERENCES discussions(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('preflight_failed','queued','running','success','failed','cancelled','timeout')),
    started_at TEXT,
    finished_at TEXT,
    duration_ms INTEGER,
    result_json TEXT,
    diagnostic TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shared_runs_discussion ON shared_runs(discussion_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_shared_runs_source ON shared_runs(kind, source_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_shared_runs_project ON shared_runs(project_id, created_at DESC);
