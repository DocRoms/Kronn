-- Per-project folders ignored by the read-only source browser and its search.
-- Kept outside the project row so the list remains small, queryable and easy
-- to replace atomically from the UI.
CREATE TABLE IF NOT EXISTS project_source_exclusions (
    project_id TEXT NOT NULL,
    path TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (project_id, path),
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_project_source_exclusions_project
    ON project_source_exclusions(project_id);
