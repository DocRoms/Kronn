-- KT-140 — multiple durable workspaces per discussion.
--
-- Keep the legacy discussions.workspace_path/worktree_branch columns during
-- the transition: existing Isolated discussions continue to use the exact
-- same runtime path while the new table exposes an additive multi-worktree
-- model for joined CLI sessions.

CREATE TABLE discussion_workspaces (
    id                  TEXT PRIMARY KEY,
    disc_id             TEXT NOT NULL,
    session_pk          INTEGER,
    task_id             TEXT,
    project_id          TEXT NOT NULL,
    workspace_path      TEXT,
    canonical_path      TEXT,
    branch              TEXT NOT NULL,
    head_sha            TEXT,
    ownership           TEXT NOT NULL
                            CHECK (ownership IN ('managed', 'external')),
    state               TEXT NOT NULL
                            CHECK (state IN ('attached', 'detached', 'missing')),
    created_at          DATETIME NOT NULL,
    updated_at          DATETIME NOT NULL,
    FOREIGN KEY (disc_id) REFERENCES discussions(id) ON DELETE CASCADE,
    FOREIGN KEY (session_pk) REFERENCES discussion_sessions(id) ON DELETE SET NULL,
    FOREIGN KEY (task_id) REFERENCES planning_tasks(id) ON DELETE SET NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

-- A live CLI session declares one current workspace. Re-declaration updates
-- that row instead of accumulating stale paths.
CREATE UNIQUE INDEX idx_discussion_workspaces_session
    ON discussion_workspaces(disc_id, session_pk)
    WHERE session_pk IS NOT NULL;

-- One physical checkout cannot be owned by two discussions at once.
CREATE UNIQUE INDEX idx_discussion_workspaces_canonical_path
    ON discussion_workspaces(canonical_path)
    WHERE canonical_path IS NOT NULL;

CREATE INDEX idx_discussion_workspaces_disc
    ON discussion_workspaces(disc_id, updated_at DESC);
CREATE INDEX idx_discussion_workspaces_task
    ON discussion_workspaces(task_id)
    WHERE task_id IS NOT NULL;

-- Preserve every legacy Isolated workspace, including unlocked worktrees
-- (workspace_path NULL but branch retained). The deterministic id keeps this
-- backfill idempotent when a fixture applies the SQL more than once.
INSERT OR IGNORE INTO discussion_workspaces (
    id, disc_id, session_pk, task_id, project_id,
    workspace_path, canonical_path, branch, head_sha,
    ownership, state, created_at, updated_at
)
SELECT
    d.id || ':legacy',
    d.id,
    NULL,
    NULL,
    d.project_id,
    d.workspace_path,
    d.workspace_path,
    d.worktree_branch,
    NULL,
    'managed',
    CASE WHEN d.workspace_path IS NULL THEN 'detached' ELSE 'attached' END,
    d.created_at,
    d.updated_at
FROM discussions d
WHERE d.project_id IS NOT NULL
  AND d.worktree_branch IS NOT NULL;
