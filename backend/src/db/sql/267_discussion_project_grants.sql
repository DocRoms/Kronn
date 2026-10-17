-- 0.15.1 (KT-1270) — projects a room may reach besides its own.
--
-- A room keeps one primary project (`discussions.project_id`): its working
-- directory, where its creations land, the only project whose credentials it
-- can use. A human may grant it read or write access to other projects, or to
-- all of them. Grants are only ever made by a human route; an agent can only
-- file a request. Only the room's own agents reach its grants, never a
-- workflow step or a worker posting in it.
--
-- A grant row is never deleted: revoking stamps it, and a new grant is a new
-- row, so the table is the journal of who opened what, when. No foreign key
-- cascades: deleting a room or a project keeps the journal (its rows then
-- reach nothing, the room's tokens being dead and the project gone).
CREATE TABLE discussion_project_grants (
    id              TEXT PRIMARY KEY,
    discussion_id   TEXT NOT NULL,
    -- NULL = every project.
    project_id      TEXT,
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

-- An agent's request. `project_id` is NULL when the name it gave matches no
-- single project: the request is recorded the same way, so the reply never
-- tells the agent whether a project it cannot read exists.
CREATE TABLE discussion_scope_requests (
    id                  TEXT PRIMARY KEY,
    discussion_id       TEXT NOT NULL,
    project_id          TEXT,
    requested_name      TEXT NOT NULL,
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
    ON discussion_scope_requests(discussion_id, COALESCE(project_id, 'name:' || requested_name))
    WHERE status = 'pending';

-- The projects a workflow's runs reach (KT-1270 DoD: scope declared at
-- launch). Only a human sets them, and they hold only while the workflow is
-- unchanged: `workflow_fingerprint` is the digest of the workflow's stored
-- definition when the human validated it; any later edit, by an agent or a
-- human, suspends the scope until a human validates it again.
CREATE TABLE workflow_scopes (
    workflow_id             TEXT NOT NULL,
    project_id              TEXT NOT NULL,
    access                  TEXT NOT NULL CHECK (access IN ('read', 'write')),
    workflow_fingerprint    TEXT NOT NULL,
    set_by                  TEXT NOT NULL,
    set_at                  TEXT NOT NULL,
    PRIMARY KEY (workflow_id, project_id)
);
