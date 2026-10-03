-- Local identity table generalising artifact_import_origins (migration 188,
-- ADR-005 slice 3): (project, kind, slug) -> id. Unlike the table it
-- replaces, this one is project-scoped and holds at most one target per key,
-- so re-importing the same slug always resolves and updates the same local
-- resource instead of accumulating an unrelated copy.
CREATE TABLE resource_identities (
    project_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    slug TEXT NOT NULL,
    target_id TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (project_key, kind, slug)
);

-- Best-effort carry-over: the old table had no project dimension, so every
-- prior mapping lands in the global ("") scope. Ambiguous (kind, source_id)
-- groups keep one deterministic target rather than failing the migration.
INSERT OR REPLACE INTO resource_identities (project_key, kind, slug, target_id, updated_at)
SELECT '', kind, source_id, MAX(target_id), datetime('now')
FROM artifact_import_origins
GROUP BY kind, source_id;

DROP TABLE artifact_import_origins;
