-- KT-1492: the trust level a human set for the instance, a workflow or a room.
-- A missing row means "inherit": the narrowest explicit setting wins
-- (room, then workflow, then instance, then Standard). No foreign key: a level
-- never outlives its scope by accident, and the journal below outlives both.
CREATE TABLE IF NOT EXISTS trust_settings (
    scope     TEXT NOT NULL CHECK (scope IN ('instance', 'workflow', 'room')),
    scope_id  TEXT NOT NULL DEFAULT '',
    level     TEXT NOT NULL CHECK (level IN ('standard', 'trusted', 'full')),
    set_by    TEXT NOT NULL,
    set_at    TEXT NOT NULL,
    PRIMARY KEY (scope, scope_id)
);

-- KT-1492: the journal. One row per approval a trust level granted on its own
-- ('grant') and per change of a setting ('setting'). Never deleted, never
-- cascaded: deleting a room, a workflow or a project keeps it.
CREATE TABLE IF NOT EXISTS trust_audit_log (
    id             TEXT PRIMARY KEY,
    at             TEXT NOT NULL,
    event          TEXT NOT NULL CHECK (event IN ('grant', 'setting')),
    approval       TEXT,
    level          TEXT NOT NULL,
    source         TEXT NOT NULL,
    source_id      TEXT,
    discussion_id  TEXT,
    workflow_id    TEXT,
    project_id     TEXT,
    subject        TEXT NOT NULL,
    detail         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_trust_audit_log_at ON trust_audit_log(at DESC);
