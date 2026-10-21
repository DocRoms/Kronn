-- KT-1346: a room without a project may be granted « General · write » by a
-- human (source `general`, no project). SQLite cannot widen a CHECK in place:
-- the table is rebuilt with its rows, journal included.
CREATE TABLE discussion_project_grants_v2 (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL,
    -- NULL = every project, or the general resources (source `general`).
    project_id      TEXT,
    access          TEXT NOT NULL CHECK (access IN ('read', 'write')),
    source          TEXT NOT NULL CHECK (source IN ('link', 'request', 'all', 'general')),
    request_id      TEXT,
    granted_by      TEXT NOT NULL,
    granted_at      TEXT NOT NULL,
    revoked_by      TEXT,
    revoked_at      TEXT,
    revoked_reason  TEXT,
    CHECK ((project_id IS NULL) = (source IN ('all', 'general'))),
    CHECK (source != 'general' OR access = 'write')
);

INSERT INTO discussion_project_grants_v2
    SELECT id, discussion_id, project_id, access, source, request_id, granted_by,
           granted_at, revoked_by, revoked_at, revoked_reason
    FROM discussion_project_grants;

DROP TABLE discussion_project_grants;
ALTER TABLE discussion_project_grants_v2 RENAME TO discussion_project_grants;

CREATE UNIQUE INDEX idx_discussion_project_grants_active
    ON discussion_project_grants(discussion_id, COALESCE(project_id, '*'))
    WHERE revoked_at IS NULL;
