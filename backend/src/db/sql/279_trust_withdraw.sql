-- KT-1492: lowering a trust level withdraws what it granted. The journal now
-- names the object each grant approved (kind, id, and a reference such as a
-- step name), keeps a grant `active` while its object still holds the
-- approval, and gains a `withdrawn` event. SQLite cannot widen a CHECK in
-- place: the table is rebuilt with its rows.
CREATE TABLE trust_audit_log_v2 (
    id             TEXT PRIMARY KEY,
    at             TEXT NOT NULL,
    event          TEXT NOT NULL CHECK (event IN ('grant', 'setting', 'withdrawn')),
    approval       TEXT,
    level          TEXT NOT NULL,
    source         TEXT NOT NULL,
    source_id      TEXT,
    discussion_id  TEXT,
    workflow_id    TEXT,
    project_id     TEXT,
    subject        TEXT NOT NULL,
    detail         TEXT NOT NULL DEFAULT '',
    -- What the grant approved: `quick_exec` (id), `workflow_step` (workflow id,
    -- step name in `object_ref`), `room_scope_grant` (grant id, room in
    -- `object_ref`). NULL for a grant with no object (a validation's reach).
    object_kind    TEXT,
    object_id      TEXT,
    object_ref     TEXT,
    -- 1 while the object still holds the approval: such a row is never pruned.
    active         INTEGER NOT NULL DEFAULT 0
);

INSERT INTO trust_audit_log_v2
    (id, at, event, approval, level, source, source_id, discussion_id, workflow_id,
     project_id, subject, detail)
    SELECT id, at, event, approval, level, source, source_id, discussion_id, workflow_id,
           project_id, subject, detail
    FROM trust_audit_log;

DROP TABLE trust_audit_log;
ALTER TABLE trust_audit_log_v2 RENAME TO trust_audit_log;

CREATE INDEX idx_trust_audit_log_at ON trust_audit_log(at DESC);
CREATE INDEX idx_trust_audit_log_active ON trust_audit_log(source, source_id) WHERE active = 1;

-- KT-1492: a Quick Exec line approved by a trust level, not by a human, runs
-- under the level's OS sandbox.
ALTER TABLE quick_execs ADD COLUMN approved_by_trust INTEGER NOT NULL DEFAULT 0;

-- KT-1492: a deleted room or workflow takes its trust setting with it, whatever
-- path deleted it (the journal stays, minus the right to withdraw: the scope is
-- gone).
CREATE TRIGGER IF NOT EXISTS trust_forget_room AFTER DELETE ON discussions
BEGIN
    DELETE FROM trust_settings WHERE scope = 'room' AND scope_id = OLD.id;
    UPDATE trust_audit_log SET active = 0
        WHERE active = 1 AND source = 'room' AND source_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS trust_forget_workflow AFTER DELETE ON workflows
BEGIN
    DELETE FROM trust_settings WHERE scope = 'workflow' AND scope_id = OLD.id;
    UPDATE trust_audit_log SET active = 0
        WHERE active = 1 AND source = 'workflow' AND source_id = OLD.id;
END;
