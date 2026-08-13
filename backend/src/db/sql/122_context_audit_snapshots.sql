-- KT-260 — one durable Context Architecture Audit baseline per project.
--
-- The GET endpoint swaps this snapshot only when the user opens or refreshes
-- the project overview.  There is deliberately no timer: drift is measured on
-- a real inspection event, never by a background loop that repeatedly walks a
-- repository.
CREATE TABLE context_audit_snapshots (
    project_id   TEXT PRIMARY KEY,
    audit_json   TEXT NOT NULL,
    captured_at  DATETIME NOT NULL,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);
