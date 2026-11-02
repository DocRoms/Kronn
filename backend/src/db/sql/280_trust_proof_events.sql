-- KT-1492: the journal also records the use (`proof`) and the refusal
-- (`proof_refused`) of a strong-confirmation code, with the channel it came
-- through (ui, http, bridge), the route and the object it was spent on. SQLite
-- cannot widen a CHECK in place: the table is rebuilt with its rows.
-- The two triggers of 279 name the table: they are dropped before it and
-- created again after.
DROP TRIGGER IF EXISTS trust_forget_room;
DROP TRIGGER IF EXISTS trust_forget_workflow;

CREATE TABLE trust_audit_log_v3 (
    id             TEXT PRIMARY KEY,
    at             TEXT NOT NULL,
    event          TEXT NOT NULL CHECK (event IN ('grant', 'setting', 'withdrawn', 'proof', 'proof_refused')),
    approval       TEXT,
    level          TEXT NOT NULL,
    source         TEXT NOT NULL,
    source_id      TEXT,
    discussion_id  TEXT,
    workflow_id    TEXT,
    project_id     TEXT,
    subject        TEXT NOT NULL,
    detail         TEXT NOT NULL DEFAULT '',
    object_kind    TEXT,
    object_id      TEXT,
    object_ref     TEXT,
    active         INTEGER NOT NULL DEFAULT 0
);

INSERT INTO trust_audit_log_v3
    (id, at, event, approval, level, source, source_id, discussion_id, workflow_id,
     project_id, subject, detail, object_kind, object_id, object_ref, active)
    SELECT id, at, event, approval, level, source, source_id, discussion_id, workflow_id,
           project_id, subject, detail, object_kind, object_id, object_ref, active
    FROM trust_audit_log;

DROP TABLE trust_audit_log;
ALTER TABLE trust_audit_log_v3 RENAME TO trust_audit_log;

CREATE INDEX idx_trust_audit_log_at ON trust_audit_log(at DESC);
CREATE INDEX idx_trust_audit_log_active ON trust_audit_log(source, source_id) WHERE active = 1;

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
