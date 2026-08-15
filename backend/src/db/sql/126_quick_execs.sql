-- Saved, reusable shell-free CLI data collectors.
CREATE TABLE IF NOT EXISTS quick_execs (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    description         TEXT NOT NULL DEFAULT '',
    icon                TEXT NOT NULL DEFAULT '⌘',
    project_id          TEXT REFERENCES projects(id) ON DELETE SET NULL,
    command             TEXT NOT NULL,
    args_json           TEXT NOT NULL DEFAULT '[]',
    timeout_secs        INTEGER NOT NULL DEFAULT 60,
    output_format       TEXT NOT NULL DEFAULT 'json'
                            CHECK(output_format IN ('json', 'text', 'lines', 'csv')),
    variables_json      TEXT NOT NULL DEFAULT '[]',
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_quick_execs_project ON quick_execs(project_id);
