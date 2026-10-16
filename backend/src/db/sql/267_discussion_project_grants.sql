-- 0.15.1 (KT-1270) — projects a room may reach besides its own.
--
-- A room keeps one primary project (`discussions.project_id`): its working
-- directory, where its creations land, the only project whose credentials it
-- can use. A human may grant it read or write access to other projects, or to
-- all of them. Grants are only ever made by a human route; an agent can only
-- file a request.
--
-- A grant row is never deleted: revoking stamps it, and a new grant is a new
-- row, so the table is the journal of who opened what, when.
CREATE TABLE discussion_project_grants (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    -- NULL = every project.
    project_id      TEXT REFERENCES projects(id) ON DELETE CASCADE,
    access          TEXT NOT NULL CHECK (access IN ('read', 'write')),
    source          TEXT NOT NULL CHECK (source IN ('link', 'request', 'all')),
    request_id      TEXT,
    granted_by      TEXT NOT NULL,
    granted_at      TEXT NOT NULL,
    revoked_by      TEXT,
    revoked_at      TEXT,
    revoked_reason  TEXT,
    CHECK ((project_id IS NULL) = (source = 'all'))
);

CREATE UNIQUE INDEX idx_discussion_project_grants_active
    ON discussion_project_grants(discussion_id, COALESCE(project_id, '*'))
    WHERE revoked_at IS NULL;

CREATE TABLE discussion_scope_requests (
    id                  TEXT PRIMARY KEY,
    discussion_id       TEXT NOT NULL REFERENCES discussions(id) ON DELETE CASCADE,
    project_id          TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    access              TEXT NOT NULL CHECK (access IN ('read', 'write')),
    reason              TEXT NOT NULL,
    requested_by        TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'declined')),
    decided_by          TEXT,
    decided_at          TEXT,
    granted_access      TEXT CHECK (granted_access IS NULL OR granted_access IN ('read', 'write')),
    created_at          TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_discussion_scope_requests_pending
    ON discussion_scope_requests(discussion_id, project_id)
    WHERE status = 'pending';
