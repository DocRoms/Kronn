CREATE TABLE project_dependency_monitoring (
    project_id TEXT PRIMARY KEY
        REFERENCES projects(id) ON DELETE CASCADE,
    interval_days INTEGER
        CHECK (interval_days IS NULL OR interval_days BETWEEN 1 AND 365),
    manifest_fingerprint TEXT,
    summary_json TEXT,
    checked_at TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
