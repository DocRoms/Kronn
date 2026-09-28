-- "Use in Kronn" for a native skill outside kronn/: a project-scoped
-- pointer by repository-relative path. Read fresh from the source on every
-- use; deliberately not part of kronn.lock (KT-897).
CREATE TABLE project_skill_references (
    project_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (project_id, slug)
);
