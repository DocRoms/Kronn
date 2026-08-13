-- KT-244 — advisory mutual exclusion for destructive Git history rewrites.
--
-- Several joined CLI sessions may intentionally share one physical worktree
-- inside ONE discussion.  The old global UNIQUE index made that legitimate
-- declaration impossible, while still not protecting the actual destructive
-- operation.  Cross-discussion ownership remains exclusive through triggers;
-- same-discussion rows are allowed and become the input to the lease guard.

DROP INDEX IF EXISTS idx_discussion_workspaces_canonical_path;

CREATE INDEX IF NOT EXISTS idx_discussion_workspaces_canonical_path
    ON discussion_workspaces(canonical_path)
    WHERE canonical_path IS NOT NULL;

CREATE TRIGGER IF NOT EXISTS trg_discussion_workspace_cross_room_insert
BEFORE INSERT ON discussion_workspaces
WHEN NEW.canonical_path IS NOT NULL
 AND EXISTS (
     SELECT 1 FROM discussion_workspaces existing
      WHERE existing.canonical_path = NEW.canonical_path
        AND existing.disc_id != NEW.disc_id
 )
BEGIN
    SELECT RAISE(ABORT, 'workspace is already declared by another discussion');
END;

CREATE TRIGGER IF NOT EXISTS trg_discussion_workspace_cross_room_update
BEFORE UPDATE OF canonical_path, disc_id ON discussion_workspaces
WHEN NEW.canonical_path IS NOT NULL
 AND EXISTS (
     SELECT 1 FROM discussion_workspaces existing
      WHERE existing.canonical_path = NEW.canonical_path
        AND existing.disc_id != NEW.disc_id
        AND existing.id != NEW.id
 )
BEGIN
    SELECT RAISE(ABORT, 'workspace is already declared by another discussion');
END;

CREATE TABLE discussion_workspace_history_leases (
    id              TEXT PRIMARY KEY,
    disc_id         TEXT NOT NULL,
    session_pk      INTEGER NOT NULL,
    canonical_path  TEXT NOT NULL,
    branch          TEXT NOT NULL,
    backup_ref      TEXT NOT NULL,
    head_sha        TEXT NOT NULL,
    acquired_at     DATETIME NOT NULL,
    expires_at      DATETIME NOT NULL,
    released_at     DATETIME,
    release_reason  TEXT,
    FOREIGN KEY (disc_id) REFERENCES discussions(id) ON DELETE CASCADE,
    FOREIGN KEY (session_pk) REFERENCES discussion_sessions(id) ON DELETE CASCADE
);

-- Expired rows are retired transactionally before a new acquire.  Keeping a
-- single unreleased row lets SQLite arbitrate two simultaneous acquisitions.
CREATE UNIQUE INDEX idx_discussion_workspace_history_lease_active
    ON discussion_workspace_history_leases(canonical_path, branch)
    WHERE released_at IS NULL;

CREATE INDEX idx_discussion_workspace_history_lease_session
    ON discussion_workspace_history_leases(session_pk, released_at);
