-- KT-1491 (#411) — a room's access to another project has three levels:
-- `read`, `plan` (what `write` always was: Kronn's planning tools, never the
-- files) and `code` (the agent edits that project's checkout like its own).
-- Stored `write` becomes `plan`; the parser still reads `write`. SQLite cannot
-- widen a CHECK in place, so each table is rebuilt with its rows, journal
-- included. `code` names one project: never every project, never a workflow's
-- scope (a run is given no checkout).

CREATE TABLE discussion_project_grants_v3 (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL,
    -- NULL = every project, or the general resources (source `general`).
    project_id      TEXT,
    access          TEXT NOT NULL CHECK (access IN ('read', 'plan', 'code')),
    source          TEXT NOT NULL CHECK (source IN ('link', 'request', 'all', 'general')),
    request_id      TEXT,
    granted_by      TEXT NOT NULL,
    granted_at      TEXT NOT NULL,
    revoked_by      TEXT,
    revoked_at      TEXT,
    revoked_reason  TEXT,
    CHECK ((project_id IS NULL) = (source IN ('all', 'general'))),
    CHECK (source != 'general' OR access = 'plan'),
    CHECK (access != 'code' OR project_id IS NOT NULL)
);

INSERT INTO discussion_project_grants_v3
    SELECT id, discussion_id, project_id,
           CASE access WHEN 'write' THEN 'plan' ELSE access END,
           source, request_id, granted_by, granted_at, revoked_by, revoked_at, revoked_reason
    FROM discussion_project_grants;

DROP TABLE discussion_project_grants;
ALTER TABLE discussion_project_grants_v3 RENAME TO discussion_project_grants;

CREATE UNIQUE INDEX idx_discussion_project_grants_active
    ON discussion_project_grants(discussion_id, COALESCE(project_id, '*'))
    WHERE revoked_at IS NULL;

CREATE TABLE discussion_scope_requests_v2 (
    id                  TEXT PRIMARY KEY,
    discussion_id       TEXT NOT NULL,
    project_id          TEXT,
    requested_name      TEXT NOT NULL,
    access              TEXT NOT NULL CHECK (access IN ('read', 'plan', 'code')),
    reason              TEXT NOT NULL,
    requested_by        TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'approved', 'declined')),
    decided_by          TEXT,
    decided_at          TEXT,
    granted_access      TEXT CHECK (granted_access IS NULL OR granted_access IN ('read', 'plan', 'code')),
    created_at          TEXT NOT NULL
);

INSERT INTO discussion_scope_requests_v2
    SELECT id, discussion_id, project_id, requested_name,
           CASE access WHEN 'write' THEN 'plan' ELSE access END,
           reason, requested_by, status, decided_by, decided_at,
           CASE granted_access WHEN 'write' THEN 'plan' ELSE granted_access END,
           created_at
    FROM discussion_scope_requests;

DROP TABLE discussion_scope_requests;
ALTER TABLE discussion_scope_requests_v2 RENAME TO discussion_scope_requests;

CREATE UNIQUE INDEX idx_discussion_scope_requests_pending
    ON discussion_scope_requests(discussion_id, COALESCE(project_id, 'name:' || requested_name))
    WHERE status = 'pending';

CREATE TABLE workflow_scopes_v2 (
    workflow_id             TEXT NOT NULL,
    project_id              TEXT NOT NULL,
    access                  TEXT NOT NULL CHECK (access IN ('read', 'plan')),
    workflow_fingerprint    TEXT NOT NULL,
    set_by                  TEXT NOT NULL,
    set_at                  TEXT NOT NULL,
    PRIMARY KEY (workflow_id, project_id)
);

INSERT INTO workflow_scopes_v2
    SELECT workflow_id, project_id,
           CASE access WHEN 'write' THEN 'plan' ELSE access END,
           workflow_fingerprint, set_by, set_at
    FROM workflow_scopes;

DROP TABLE workflow_scopes;
ALTER TABLE workflow_scopes_v2 RENAME TO workflow_scopes;
